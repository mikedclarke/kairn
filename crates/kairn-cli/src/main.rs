//! `kairn`: the notes from the command line, for people, scripts, and
//! agents. Every write goes through kairn-core's never-clobber atomic
//! paths and lands in the `.kairn/activity.jsonl` log the app renders.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{Duration, Local, NaiveDate};
use clap::{ArgAction, Parser, Subcommand};
use kairn_core::{
    ActivityEntry, Mention, SearchHit, TaskQuery, TaskRef, WikiTarget, daily_file,
    fuzzy_score, log_activity, mentions_of, open_tasks_in_dailies, resolve_wiki_target,
    search_notes, settings::Settings, toggle_task_on_disk, vault, write,
};
use serde_json::json;

/// Exit codes are part of the interface: scripts and agents branch on them.
const EXIT_FAILED: u8 = 1;
const EXIT_NOT_FOUND: u8 = 3;
const EXIT_AMBIGUOUS: u8 = 4;

#[derive(Parser)]
#[command(
    name = "kairn",
    version,
    about = "Read, search, and update a Kairn notes folder from the command line.",
    long_about = "\
Read, search, and update a Kairn notes folder from the command line.

The notes are plain markdown files in a NotePlan-compatible layout (daily
notes in Calendar/, everything else in Notes/), so reading a whole note
needs no tooling: `cat` the file. This CLI is for what needs Kairn's
semantics: parsed tasks, fuzzy search, backlinks, and safe writes that
never clobber concurrent edits.

Every command takes --json for stable machine-readable output. Writes are
recorded in the notes folder's activity log, which the Kairn app shows in
its sidebar.",
    after_help = "\
Exit codes:
  0  success
  1  failed (I/O error, missing notes folder)
  2  bad usage (unknown flag or argument)
  3  nothing matched the given title, task, or date
  4  more than one task matched; add more words to pick one

Examples:
  kairn today                          print today's daily note
  kairn add \"email Sam about invoice\"  add a task to today
  kairn add \"pay VAT\" --date 2026-09-01
  kairn done \"email sam\"               mark the matching task done
  kairn tasks --overdue --json         overdue tasks, as JSON
  kairn search \"sim test\"              find notes about it
  kairn note \"Kairn PRD\"               print a note by title
  kairn backlinks \"Kairn PRD\"          lines that link to it
  kairn capture \"idea: agents view\"    quick-append to today"
)]
struct Cli {
    /// Notes folder to use (default: the app's configured folder, else ~/kairn)
    #[arg(long, global = true, env = "KAIRN_ROOT", value_name = "DIR")]
    root: Option<PathBuf>,

    /// Print machine-readable JSON instead of plain text
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    json: bool,

    /// Name recorded in the activity log for writes (default: cli)
    #[arg(long, global = true, env = "KAIRN_ACTOR", value_name = "NAME")]
    actor: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print today's daily note
    #[command(long_about = "\
Print today's daily note as markdown. Prints nothing (and stays exit 0)
when today has no note yet; `kairn add` or `kairn capture` starts one.")]
    Today,

    /// Print a note by title
    #[command(long_about = "\
Print a note as markdown, resolved the way [[wiki links]] resolve:
a title matches a note under Notes/ case-insensitively (folder-qualified
titles like Projects/Kairn work), an ISO date like 2026-08-12 is that
day's daily note, and a period like 2026-W32 or 2026-08 is that period's
note. Exit 3 with suggestions when nothing has that title.")]
    Note {
        /// Note title, ISO date (2026-08-12), or period (2026-W32)
        title: String,
        /// Print the note's file path instead of its content
        #[arg(long, action = ArgAction::SetTrue)]
        path: bool,
    },

    /// List open tasks from the daily notes
    #[command(long_about = "\
List open tasks across every daily note, newest day first, one per line
as `DATE  TEXT`. A task's date is the daily note it lives in. Done,
scheduled ([>]), and cancelled ([-]) tasks never appear.")]
    Tasks {
        /// Only tasks on today's note
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "overdue")]
        today: bool,
        /// Only tasks on notes from days before today
        #[arg(long, action = ArgAction::SetTrue)]
        overdue: bool,
    },

    /// Add a task to a daily note
    #[command(long_about = "\
Append TEXT as an open task (`* TEXT`) to a daily note, creating the note
from the daily template if the day has none yet. Defaults to today; --date
picks another day.")]
    Add {
        /// The task text, quoted
        text: String,
        /// Day to add to: today, tomorrow, 2026-08-12, or \"aug 12\"
        #[arg(long, value_name = "WHEN", default_value = "today")]
        date: String,
    },

    /// Mark an open task as done
    #[command(long_about = "\
Find the one open task whose text contains MATCH (case-insensitive) and
mark it done with a NotePlan `@done(...)` timestamp. If several tasks
match, nothing is changed: the matches are listed and the exit code is 4;
rerun with more of the task's words. Exit 3 when nothing matches.")]
    Done {
        /// Words from the task's text, quoted
        r#match: String,
    },

    /// Quick-capture a line into today's note
    #[command(long_about = "\
Append TEXT to today's daily note as an open task, creating the note from
the daily template if needed. The same quick-capture the app's Capture
button does; for another day, use `kairn add --date`.")]
    Capture {
        /// The text to capture, quoted
        text: String,
    },

    /// Search every note
    #[command(long_about = "\
Search all notes: fuzzy title matches first (typing `kprd` finds
`kairn-prd`), then notes whose body contains QUERY as a plain substring,
with one matching line shown per note. Case-insensitive.")]
    Search {
        /// What to look for
        query: String,
        /// Most results to print
        #[arg(long, value_name = "N", default_value_t = 20)]
        limit: usize,
    },

    /// List lines that link to a note
    #[command(long_about = "\
Print every line elsewhere that references TITLE via a [[wiki link]]
(plus `>date` schedule references when TITLE is a day's ISO date). One line
per mention: `SOURCE: LINE`.")]
    Backlinks {
        /// Note title, or ISO date for a day's backlinks
        title: String,
    },
}

/// A failure with its exit code, already explained. `hint` tells the caller
/// what to do instead, the error-message quality bar for this binary.
#[derive(Debug)]
struct Failure {
    code: u8,
    message: String,
    hint: Option<String>,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("kairn: {}", f.message);
            if let Some(hint) = &f.hint {
                eprintln!("kairn: {hint}");
            }
            if json {
                // Stdout stays parseable even on failure.
                let body = json!({ "error": f.message, "hint": f.hint });
                println!("{body}");
            }
            ExitCode::from(f.code)
        }
    }
}

fn run(cli: Cli) -> Result<(), Failure> {
    let root = notes_root(cli.root.as_deref())?;
    let actor = cli.actor.unwrap_or_else(|| "cli".to_string());
    match cli.command {
        Command::Today => cmd_today(&root, cli.json),
        Command::Note { title, path } => cmd_note(&root, &title, path, cli.json),
        Command::Tasks { today, overdue } => {
            let query = match (today, overdue) {
                (true, _) => TaskQuery::Today,
                (_, true) => TaskQuery::Overdue,
                _ => TaskQuery::Open,
            };
            cmd_tasks(&root, query, cli.json)
        }
        Command::Add { text, date } => {
            let date = parse_when(&date)?;
            cmd_append(&root, date, &text, "add", &actor, cli.json)
        }
        Command::Capture { text } => {
            cmd_append(&root, Local::now().date_naive(), &text, "capture", &actor, cli.json)
        }
        Command::Done { r#match } => cmd_done(&root, &r#match, &actor, cli.json),
        Command::Search { query, limit } => cmd_search(&root, &query, limit, cli.json),
        Command::Backlinks { title } => cmd_backlinks(&root, &title, cli.json),
    }
}

/// The notes root, validated to exist: `--root`/`$KAIRN_ROOT` wins, else
/// the app's configured folder. Nothing here creates folders: pointing at
/// a typo'd path must fail loudly, not scaffold an empty vault there.
fn notes_root(flag: Option<&Path>) -> Result<PathBuf, Failure> {
    let root = match flag {
        Some(path) => path.to_path_buf(),
        None => Settings::load().notes_root(),
    };
    if !root.is_dir() {
        return Err(Failure::new(
            EXIT_FAILED,
            format!("notes folder {} does not exist", root.display()),
        )
        .hint(
            "set it in the Kairn app's settings, pass --root DIR, or set KAIRN_ROOT",
        ));
    }
    Ok(root)
}

/// `--date` values: `today`, `tomorrow`, `yesterday`, ISO `2026-08-12`, or
/// a human day like `aug 12` (this year when no year is given).
fn parse_when(when: &str) -> Result<NaiveDate, Failure> {
    let today = Local::now().date_naive();
    match when.trim().to_lowercase().as_str() {
        "today" => return Ok(today),
        "tomorrow" => return Ok(today + Duration::days(1)),
        "yesterday" => return Ok(today - Duration::days(1)),
        _ => {}
    }
    vault::parse_day_query(when, today).ok_or_else(|| {
        Failure::new(2, format!("{when:?} is not a date I understand"))
            .hint("use today, tomorrow, yesterday, 2026-08-12, or \"aug 12\"")
    })
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

/// The task's own words: the raw line with indentation, list marker, and
/// the open bracket stripped. What matching and display both use.
fn task_text(line: &str) -> &str {
    let s = line.trim_start();
    let s = ["* ", "+ ", "- "]
        .iter()
        .find_map(|m| s.strip_prefix(m))
        .unwrap_or(s);
    let s = s.trim_start();
    s.strip_prefix("[ ]").map(str::trim_start).unwrap_or(s)
}

fn read_note(path: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|e| {
        Failure::new(EXIT_FAILED, format!("could not read {}: {e}", path.display()))
    })
}

fn cmd_today(root: &Path, json: bool) -> Result<(), Failure> {
    let date = Local::now().date_naive();
    let file = daily_file(root, date);
    let content = match &file {
        Some(path) => read_note(path)?,
        None => String::new(),
    };
    if json {
        let body = json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "file": file.as_ref().map(|p| rel(root, p)),
            "content": content,
        });
        println!("{body}");
    } else if file.is_none() {
        eprintln!("kairn: no daily note for today yet; kairn add \"...\" starts one");
    } else {
        print!("{content}");
    }
    Ok(())
}

fn cmd_note(root: &Path, title: &str, path_only: bool, json: bool) -> Result<(), Failure> {
    let path = match resolve_wiki_target(root, title) {
        WikiTarget::Day(date) => daily_file(root, date).ok_or_else(|| {
            Failure::new(
                EXIT_NOT_FOUND,
                format!("no daily note for {}", date.format("%Y-%m-%d")),
            )
            .hint(format!(
                "kairn add \"...\" --date {} starts one",
                date.format("%Y-%m-%d")
            ))
        })?,
        WikiTarget::Note(path) => path,
        WikiTarget::Missing(_) => {
            let mut failure =
                Failure::new(EXIT_NOT_FOUND, format!("no note called {title:?}"));
            let near: Vec<String> = search_notes(root, title, 4)
                .into_iter()
                .filter(|hit| hit.snippet.is_none())
                .map(|hit| hit.name)
                .collect();
            if !near.is_empty() {
                failure = failure.hint(format!("close titles: {}", near.join(", ")));
            }
            return Err(failure);
        }
        WikiTarget::Invalid => {
            return Err(Failure::new(2, format!("{title:?} cannot name a note"))
                .hint("titles cannot contain empty, dot-leading, or backslash parts"));
        }
    };
    if path_only && !json {
        println!("{}", path.display());
        return Ok(());
    }
    let content = read_note(&path)?;
    if json {
        let body = json!({
            "title": title,
            "file": rel(root, &path),
            "path": path.display().to_string(),
            "content": content,
        });
        println!("{body}");
    } else {
        print!("{content}");
    }
    Ok(())
}

fn task_json(root: &Path, task: &TaskRef) -> serde_json::Value {
    json!({
        "date": task.date.format("%Y-%m-%d").to_string(),
        "file": rel(root, &task.path),
        "line": task.line_idx + 1,
        "text": task_text(&task.line),
    })
}

fn cmd_tasks(root: &Path, query: TaskQuery, json: bool) -> Result<(), Failure> {
    let today = Local::now().date_naive();
    let tasks: Vec<TaskRef> = open_tasks_in_dailies(root)
        .into_iter()
        .filter(|t| query.matches(t.date, today))
        .collect();
    if json {
        let body = json!({
            "count": tasks.len(),
            "tasks": tasks.iter().map(|t| task_json(root, t)).collect::<Vec<_>>(),
        });
        println!("{body}");
    } else {
        for task in &tasks {
            println!("{}  {}", task.date.format("%Y-%m-%d"), task_text(&task.line));
        }
        if tasks.is_empty() {
            eprintln!("kairn: no matching open tasks");
        }
    }
    Ok(())
}

fn cmd_append(
    root: &Path,
    date: NaiveDate,
    text: &str,
    action: &str,
    actor: &str,
    json: bool,
) -> Result<(), Failure> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Failure::new(2, "nothing to add: the text is empty"));
    }
    let path = write::append_to_day(root, date, text).map_err(|e| {
        Failure::new(EXIT_FAILED, format!("could not write the daily note: {e}"))
    })?;
    log_write(root, actor, action, &path, text);
    if json {
        let body = json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "file": rel(root, &path),
            "text": text,
        });
        println!("{body}");
    } else {
        println!("added to {}", rel(root, &path));
    }
    Ok(())
}

fn cmd_done(root: &Path, needle: &str, actor: &str, json: bool) -> Result<(), Failure> {
    let needle_trim = needle.trim();
    if needle_trim.is_empty() {
        return Err(Failure::new(2, "give some words from the task's text"));
    }
    let tasks = open_tasks_in_dailies(root);
    let lower = needle_trim.to_lowercase();
    let matches: Vec<&TaskRef> = tasks
        .iter()
        .filter(|t| task_text(&t.line).to_lowercase().contains(&lower))
        .collect();
    let task = match matches.as_slice() {
        [one] => *one,
        [] => {
            let mut failure = Failure::new(
                EXIT_NOT_FOUND,
                format!("no open task contains {needle_trim:?}"),
            );
            let mut scored: Vec<(i64, &TaskRef)> = tasks
                .iter()
                .filter_map(|t| {
                    fuzzy_score(needle_trim, task_text(&t.line)).map(|s| (s, t))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            let near: Vec<String> = scored
                .iter()
                .take(3)
                .map(|(_, t)| format!("{:?}", task_text(&t.line)))
                .collect();
            if !near.is_empty() {
                failure = failure.hint(format!("did you mean: {}", near.join(", ")));
            }
            return Err(failure);
        }
        several => {
            let listed: Vec<String> = several
                .iter()
                .map(|t| format!("  {}  {}", t.date.format("%Y-%m-%d"), task_text(&t.line)))
                .collect();
            return Err(Failure::new(
                EXIT_AMBIGUOUS,
                format!(
                    "{} open tasks contain {needle_trim:?}:\n{}",
                    several.len(),
                    listed.join("\n")
                ),
            )
            .hint("rerun with more of the task's words to pick one"));
        }
    };
    let changed = toggle_task_on_disk(&task.path, task.line_idx, &task.line)
        .map_err(|e| {
            Failure::new(EXIT_FAILED, format!("could not update the note: {e}"))
        })?;
    if !changed {
        return Err(Failure::new(
            EXIT_FAILED,
            "the note changed while updating and the task could not be found again",
        )
        .hint("run the same command again"));
    }
    let text = task_text(&task.line);
    log_write(root, actor, "done", &task.path, text);
    if json {
        let body = json!({
            "date": task.date.format("%Y-%m-%d").to_string(),
            "file": rel(root, &task.path),
            "text": text,
        });
        println!("{body}");
    } else {
        println!("done: {text}");
    }
    Ok(())
}

fn cmd_search(root: &Path, query: &str, limit: usize, json: bool) -> Result<(), Failure> {
    let hits = search_notes(root, query, limit);
    if json {
        let body = json!({
            "count": hits.len(),
            "hits": hits.iter().map(|hit: &SearchHit| json!({
                "name": hit.name,
                "file": rel(root, &hit.path),
                "date": hit.date.map(|d| d.format("%Y-%m-%d").to_string()),
                "snippet": hit.snippet,
            })).collect::<Vec<_>>(),
        });
        println!("{body}");
    } else {
        for hit in &hits {
            match &hit.snippet {
                Some(snippet) => println!("{}: {snippet}", hit.name),
                None => println!("{}", hit.name),
            }
        }
        if hits.is_empty() {
            eprintln!("kairn: nothing found for {query:?}");
        }
    }
    Ok(())
}

fn cmd_backlinks(root: &Path, title: &str, json: bool) -> Result<(), Failure> {
    let mentions = mentions_of(root, title, None);
    let line_of = |m: &Mention| -> String {
        m.spans.iter().map(|(_, s)| s.as_str()).collect()
    };
    if json {
        let body = json!({
            "count": mentions.len(),
            "mentions": mentions.iter().map(|m| json!({
                "source": m.name,
                "file": rel(root, &m.path),
                "date": m.date.map(|d| d.format("%Y-%m-%d").to_string()),
                "line": line_of(m),
            })).collect::<Vec<_>>(),
        });
        println!("{body}");
    } else {
        for mention in &mentions {
            println!("{}: {}", mention.name, line_of(mention));
        }
        if mentions.is_empty() {
            eprintln!("kairn: nothing links to {title:?}");
        }
    }
    Ok(())
}

/// Record a write in the activity log. Logging must never fail the command
/// that already succeeded, so problems only warn.
fn log_write(root: &Path, actor: &str, action: &str, path: &Path, detail: &str) {
    let entry = ActivityEntry {
        ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        actor: actor.to_string(),
        action: action.to_string(),
        target: rel(root, path),
        detail: detail.to_string(),
    };
    if let Err(e) = log_activity(root, &entry) {
        eprintln!("kairn: could not write the activity log: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_text_strips_markers_and_open_brackets() {
        assert_eq!(task_text("* buy milk"), "buy milk");
        assert_eq!(task_text("  + [ ] pack bag"), "pack bag");
        assert_eq!(task_text("- [ ]  call bank"), "call bank");
        // Content brackets past the marker stay content.
        assert_eq!(task_text("* see [[note]]"), "see [[note]]");
    }

    #[test]
    fn when_words_and_dates_parse() {
        let today = Local::now().date_naive();
        assert_eq!(parse_when("today").unwrap(), today);
        assert_eq!(parse_when("Tomorrow").unwrap(), today + Duration::days(1));
        assert_eq!(parse_when("yesterday").unwrap(), today - Duration::days(1));
        assert_eq!(
            parse_when("2026-08-12").unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()
        );
        let failure = parse_when("someday").unwrap_err();
        assert_eq!(failure.code, 2);
    }
}
