use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use chrono::{Local, NaiveDate};
use kairn_core::{ActivityEntry, NoteLock, write};
use serde::{Deserialize, Serialize};

use crate::carry::{self, Move};

#[derive(Serialize, Deserialize)]
struct Step {
    path: PathBuf,
    before: Option<String>,
    after: String,
    complete: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Report {
    pub to: String,
    pub file: PathBuf,
    pub tasks: Vec<serde_json::Value>,
    pub older_than_window: usize,
}

#[derive(Serialize, Deserialize)]
struct Plan {
    version: u32,
    root: PathBuf,
    id: String,
    section: Option<String>,
    report: Report,
    steps: Vec<Step>,
    activity: Vec<ActivityEntry>,
}

pub struct Transfer {
    journal: PathBuf,
    _lock: NoteLock,
    plan: Plan,
}

impl Transfer {
    pub fn pending(root: &Path) -> io::Result<Option<Self>> {
        let journal = root.join(".kairn/local/carry.json");
        match fs::metadata(&journal) {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let lock = NoteLock::acquire(&journal)?;
        let Some(text) = read_optional(&lock)? else {
            return Ok(None);
        };
        let plan: Plan = serde_json::from_str(&text).map_err(io::Error::other)?;
        if plan.version != 1 || plan.root != root.canonicalize()? || plan.steps.is_empty() {
            return Err(io::Error::other(
                "carry recovery plan does not belong to this vault",
            ));
        }
        for step in &plan.steps {
            checked_note_path(&plan.root, &step.path)?;
        }
        Ok(Some(Self {
            journal,
            _lock: lock,
            plan,
        }))
    }

    pub fn matches(&self, dest: NaiveDate, section: Option<&str>) -> bool {
        self.plan.report.to == dest.format("%Y-%m-%d").to_string()
            && self.plan.section.as_deref() == section
    }

    pub fn report(&self) -> &Report {
        &self.plan.report
    }

    pub fn resume_hint(&self) -> String {
        format!(
            "finish the pending carry to {}{} before starting another",
            self.plan.report.to,
            self.plan
                .section
                .as_deref()
                .map(|section| format!(" in section {section:?}"))
                .unwrap_or_default()
        )
    }

    pub fn prepare(
        root: &Path,
        section: Option<&str>,
        actor: &str,
        template_rule: &str,
        moves: &[Move],
        texts: &HashMap<PathBuf, String>,
        report: Report,
    ) -> io::Result<Self> {
        let dest = NaiveDate::parse_from_str(&report.to, "%Y-%m-%d").map_err(io::Error::other)?;
        let canonical = root.canonicalize()?;
        let journal = root.join(".kairn/local/carry.json");
        fs::create_dir_all(journal.parent().unwrap())?;
        sync_dir(&root.join(".kairn"))?;
        sync_dir(root)?;
        let lock = NoteLock::acquire(&journal)?;
        if read_optional(&lock)?.is_some() {
            return Err(io::Error::other(
                "a carry recovery plan already exists; rerun carry to finish it",
            ));
        }
        let destination = checked_note_path(&canonical, &report.file)?;
        let mut by_file: BTreeMap<PathBuf, Vec<&Move>> = BTreeMap::new();
        for movement in moves {
            by_file
                .entry(movement.task.path.clone())
                .or_default()
                .push(movement);
        }
        let mut paths: Vec<_> = by_file.keys().cloned().collect();
        paths.push(destination.clone());
        paths.sort();
        paths.dedup();
        let locks: HashMap<_, _> = paths
            .into_iter()
            .map(|path| NoteLock::acquire(&path).map(|lock| (path, lock)))
            .collect::<io::Result<_>>()?;
        let before = read_optional(&locks[&destination])?;
        let seed = if before.is_none() && kairn_core::template_applies(template_rule, dest) {
            kairn_core::daily_template(root).unwrap_or_default()
        } else {
            String::new()
        };
        let existing = before
            .as_deref()
            .unwrap_or(kairn_core::strip_daily_title(&seed));
        let block = carry::destination_block(moves);
        let (after, _) = match section {
            Some(section) => write::append_to_section_in_text(existing, section, &block),
            None => write::append_line_in_text(existing, &block),
        };
        let mut steps = vec![Step {
            path: report.file.clone(),
            before,
            after,
            complete: false,
        }];
        for (path, file_moves) in by_file {
            let before = locks[&path].read_text()?;
            if texts.get(&path) != Some(&before) {
                return Err(changed(&path));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_owned();
            checked_note_path(&canonical, &relative)?;
            let after = remove_sources(&before, &file_moves)?;
            steps.push(Step {
                path: relative,
                before: Some(before),
                after,
                complete: false,
            });
        }
        let now = Local::now();
        let activity = moves
            .iter()
            .map(|movement| ActivityEntry {
                ts: now.format("%Y-%m-%d %H:%M:%S").to_string(),
                actor: actor.to_owned(),
                action: "carry".into(),
                target: report.file.to_string_lossy().into_owned(),
                detail: crate::task_text(&movement.carried_line).to_owned(),
            })
            .collect();
        let transfer = Self {
            journal,
            _lock: lock,
            plan: Plan {
                version: 1,
                root: canonical,
                id: format!(
                    "carry-{}-{}",
                    std::process::id(),
                    now.timestamp_nanos_opt().unwrap()
                ),
                section: section.map(str::to_owned),
                report,
                steps,
                activity,
            },
        };
        transfer.save()?;
        Ok(transfer)
    }

    pub fn apply(self) -> io::Result<Report> {
        self.apply_with_hook(|_| Ok(()))
    }

    fn apply_with_hook(
        mut self,
        mut after_write: impl FnMut(usize) -> io::Result<()>,
    ) -> io::Result<Report> {
        let mut paths: Vec<_> = self
            .plan
            .steps
            .iter()
            .map(|step| checked_note_path(&self.plan.root, &step.path))
            .collect::<io::Result<_>>()?;
        paths.sort();
        paths.dedup();
        let locks: HashMap<_, _> = paths
            .into_iter()
            .map(|path| NoteLock::acquire(&path).map(|lock| (path, lock)))
            .collect::<io::Result<_>>()?;
        for step in &self.plan.steps {
            let path = self.plan.root.join(&step.path);
            let current = read_optional(&locks[&path])?;
            if current.as_deref() != Some(&step.after) && (step.complete || current != step.before)
            {
                return Err(changed(&path));
            }
        }
        for index in 0..self.plan.steps.len() {
            let step = &self.plan.steps[index];
            let path = self.plan.root.join(&step.path);
            let lock = &locks[&path];
            let current = read_optional(lock)?;
            if current.as_deref() != Some(&step.after) {
                if step.complete || current != step.before {
                    return Err(changed(&path));
                }
                lock.write_text(&step.after)?;
            }
            after_write(index)?;
            self.plan.steps[index].complete = true;
            self.save()?;
        }
        self.record_activity()?;
        after_write(self.plan.steps.len())?;
        fs::remove_file(&self.journal)?;
        sync_dir(self.journal.parent().unwrap())?;
        Ok(self.plan.report)
    }

    fn save(&self) -> io::Result<()> {
        let parent = self.journal.parent().unwrap();
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temporary, &self.plan).map_err(io::Error::other)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.journal)
            .map_err(|error| error.error)?;
        sync_dir(parent)
    }

    fn record_activity(&self) -> io::Result<()> {
        let path = kairn_core::activity_log_path(&self.plan.root);
        let lock = NoteLock::acquire(&path)?;
        let mut text = read_optional(&lock)?.unwrap_or_default();
        if text
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|entry| entry["operation_id"].as_str() == Some(&self.plan.id))
        {
            return Ok(());
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for entry in &self.plan.activity {
            let mut entry = serde_json::to_value(entry).map_err(io::Error::other)?;
            entry["operation_id"] = self.plan.id.clone().into();
            text.push_str(&serde_json::to_string(&entry).map_err(io::Error::other)?);
            text.push('\n');
        }
        lock.write_text(&text)
    }
}

fn read_optional(lock: &NoteLock) -> io::Result<Option<String>> {
    match lock.read_text() {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn checked_note_path(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let parts: Vec<_> = relative.components().collect();
    if !matches!(parts.as_slice(), [Component::Normal(dir), Component::Normal(_)] if *dir == "Calendar")
        || !matches!(
            relative.extension().and_then(|ext| ext.to_str()),
            Some("md" | "txt")
        )
        || root.join("Calendar").canonicalize()? != root.join("Calendar")
    {
        return Err(io::Error::other("invalid note path in carry recovery plan"));
    }
    Ok(root.join(relative))
}

fn changed(path: &Path) -> io::Error {
    io::Error::other(format!(
        "carry stopped: {} changed from the saved transfer plan; review the notes and .kairn/local/carry.json before retrying",
        path.display()
    ))
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn remove_sources(before: &str, moves: &[&Move]) -> io::Result<String> {
    let mut lines: Vec<_> = before.lines().collect();
    let mut ordered = moves.to_vec();
    ordered.sort_by_key(|movement| std::cmp::Reverse(movement.task.line_idx));
    for movement in &ordered {
        let index = movement.task.line_idx;
        if lines.get(index).copied() != Some(&movement.task.line) {
            return Err(changed(&movement.task.path));
        }
        lines.remove(index);
    }
    for index in (0..lines.len()).rev() {
        if moves
            .iter()
            .any(|movement| movement.header.as_deref() == Some(lines[index]))
            && carry::header_now_empty(&lines, index)
        {
            lines.remove(index);
        }
    }
    let ending = if before.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut after = lines.join(ending);
    if !lines.is_empty() && before.ends_with('\n') {
        after.push_str(ending);
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        temp: tempfile::TempDir,
        root: PathBuf,
        dest: NaiveDate,
        moves: Vec<Move>,
        texts: HashMap<PathBuf, String>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fs::create_dir(root.join("Calendar")).unwrap();
            let dest = Local::now().date_naive();
            let mut moves = Vec::new();
            let mut texts = HashMap::new();
            for offset in [2, 1] {
                let day = dest - chrono::Duration::days(offset);
                let path = kairn_core::daily_path(&root, day);
                let text = "**Group**\n* duplicate\n".to_owned();
                fs::write(&path, &text).unwrap();
                texts.insert(path.clone(), text);
                moves.push(Move {
                    task: kairn_core::TaskRef {
                        path,
                        due: day,
                        file_date: Some(day),
                        line_idx: 1,
                        line: "* duplicate".into(),
                        spans: Vec::new(),
                    },
                    header: Some("**Group**".into()),
                    carried_line: "* duplicate".into(),
                });
            }
            fs::write(kairn_core::daily_path(&root, dest), "* duplicate\n").unwrap();
            Self {
                temp,
                root,
                dest,
                moves,
                texts,
            }
        }

        fn prepare(&self) -> Transfer {
            Transfer::prepare(
                &self.root,
                None,
                "test",
                "",
                &self.moves,
                &self.texts,
                Report {
                    to: self.dest.format("%Y-%m-%d").to_string(),
                    file: kairn_core::daily_path(Path::new(""), self.dest),
                    tasks: self
                        .moves
                        .iter()
                        .map(|movement| serde_json::json!({"text": movement.carried_line}))
                        .collect(),
                    older_than_window: 0,
                },
            )
            .unwrap()
        }

        fn assert_finished(&self) {
            assert_eq!(
                fs::read_to_string(kairn_core::daily_path(&self.root, self.dest)).unwrap(),
                "* duplicate\n**Group**\n* duplicate\n* duplicate\n"
            );
            for path in self.texts.keys() {
                assert_eq!(fs::read_to_string(path).unwrap(), "");
            }
            assert_eq!(kairn_core::recent_activity(&self.root, 10).len(), 2);
            assert!(Transfer::pending(&self.root).unwrap().is_none());
            assert!(self.temp.path().exists());
        }
    }

    #[test]
    fn every_interruption_boundary_resumes_without_duplicate_tasks_or_activity() {
        for stop_after in 0..=3 {
            let fixture = Fixture::new();
            let result = fixture.prepare().apply_with_hook(|step| {
                if step == stop_after {
                    Err(io::Error::other("interrupted"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err());
            let resumed = Transfer::pending(&fixture.root).unwrap().unwrap();
            assert!(resumed.matches(fixture.dest, None));
            resumed.apply().unwrap();
            fixture.assert_finished();
        }
    }

    #[test]
    fn changed_destination_refuses_resume_and_keeps_remaining_sources() {
        let fixture = Fixture::new();
        assert!(
            fixture
                .prepare()
                .apply_with_hook(|_| Err(io::Error::other("interrupted")))
                .is_err()
        );
        let destination = kairn_core::daily_path(&fixture.root, fixture.dest);
        let edited = format!("{}extra edit\n", fs::read_to_string(&destination).unwrap());
        fs::write(&destination, &edited).unwrap();
        assert!(
            Transfer::pending(&fixture.root)
                .unwrap()
                .unwrap()
                .apply()
                .is_err()
        );
        assert_eq!(fs::read_to_string(destination).unwrap(), edited);
        for (path, before) in &fixture.texts {
            assert_eq!(&fs::read_to_string(path).unwrap(), before);
        }
        assert!(fixture.root.join(".kairn/local/carry.json").exists());
    }

    #[test]
    fn source_change_after_planning_aborts_before_destination_write() {
        let fixture = Fixture::new();
        let transfer = fixture.prepare();
        fs::write(&fixture.moves[0].task.path, "* changed\n").unwrap();
        assert!(transfer.apply().is_err());
        assert_eq!(
            fs::read_to_string(kairn_core::daily_path(&fixture.root, fixture.dest)).unwrap(),
            "* duplicate\n"
        );
    }

    #[test]
    fn logging_failure_keeps_the_plan_until_the_activity_is_durable() {
        let fixture = Fixture::new();
        let transfer = fixture.prepare();
        let activity = kairn_core::activity_log_path(&fixture.root);
        fs::create_dir(&activity).unwrap();
        assert!(transfer.apply().is_err());
        fs::remove_dir(activity).unwrap();
        Transfer::pending(&fixture.root)
            .unwrap()
            .unwrap()
            .apply()
            .unwrap();
        fixture.assert_finished();
    }

    #[test]
    fn pending_plan_is_private_and_another_carry_cannot_own_it() {
        let fixture = Fixture::new();
        let transfer = fixture.prepare();
        assert!(Transfer::pending(&fixture.root).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&transfer.journal)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(transfer);
        assert!(Transfer::pending(&fixture.root).unwrap().is_some());
    }
}
