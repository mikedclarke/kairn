//! `kairn`: the notes from the command line, for people, scripts, and
//! agents. Every write goes through kairn-core's never-clobber atomic
//! paths and lands in the `.kairn/activity.jsonl` log the app renders.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{Duration, Local, NaiveDate};
use clap::{ArgAction, Parser, Subcommand};
use kairn_core::{
    ActivityEntry, Mention, SearchHit, TaskQuery, TaskRef, WikiTarget, daily_file,
    done_tasks_in_vault, fuzzy_score, log_activity, mentions_of, open_tasks_in_vault,
    resolve_wiki_target, search_notes, settings::Settings, toggle_task_on_disk, vault, write,
};
use serde_json::json;

mod carry;

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
  kairn tasks --done --since 2026-08-04  what got ticked recently
  kairn search \"sim test\"              find notes about it
  kairn note \"Kairn PRD\"               print a note by title
  kairn backlinks \"Kairn PRD\"          lines that link to it
  kairn capture \"idea: agents view\"    quick-append to today
  kairn append today \"decided: ship it\" --section Log
  kairn edit today --find \"email sam\" --append \"(done by phone)\"
  kairn edit 2026-08-07 --find \"old reminder\" --delete
  kairn carry --dry-run                what would move into today
  kairn recent --days 2                notes touched since yesterday"
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
day's daily note, a period like 2026-W32 or 2026-08 is that period's
note, and the words today, tomorrow, and yesterday name those days.
Exit 3 with suggestions when nothing has that title.")]
    Note {
        /// Note title, ISO date (2026-08-12), period (2026-W32), or today
        title: String,
        /// Print the note's file path instead of its content
        #[arg(long, action = ArgAction::SetTrue)]
        path: bool,
    },

    /// List open tasks, by due date
    #[command(long_about = "\
List open tasks across the whole notes folder, newest due date first, one
per line as `DUE-DATE  TEXT`. A `>2026-08-12` token on the line means the
task is due that day; a daily-note task without one is due on its note's
day. Tasks in other notes appear when they carry a `>date` token.
Scheduled ([>]) and cancelled ([-]) tasks never appear.

--done lists done ([x]) tasks instead — same population, same due-date
rules — so `kairn tasks --done --since 2026-08-04` answers what got
ticked recently.")]
    Tasks {
        /// Only tasks due today
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "overdue")]
        today: bool,
        /// Only tasks due before today
        #[arg(long, action = ArgAction::SetTrue)]
        overdue: bool,
        /// List done ([x]) tasks instead of open ones
        #[arg(long, action = ArgAction::SetTrue)]
        done: bool,
        /// Only tasks due on or after this day: today, 2026-08-01, "aug 1"
        #[arg(long, value_name = "WHEN")]
        since: Option<String>,
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
        /// Section to add into (e.g. \"Todays Tasks\"); default end of note
        #[arg(long, value_name = "HEADING")]
        section: Option<String>,
    },

    /// Mark an open task as done
    #[command(long_about = "\
Find the one open task whose text contains MATCH (case-insensitive) and
mark it done (`[x]`); the day it lives on dates it. If several tasks
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

    /// Append a line of text to a note
    #[command(long_about = "\
Append TEXT to a note as-is: no task marker is added (that is `kairn add`).
NOTE resolves like `kairn note`: a title, ISO date, period, or
today/tomorrow/yesterday. A daily note that doesn't exist yet is created
(from the daily template when configured); a regular note must already
exist.

With --section, the text lands at the end of the named section instead of
the end of the note. The section is matched by heading text — `#` marks,
`==` highlights, and case are all ignored, so --section \"todays tasks\"
finds `### ==Todays Tasks==`. When no heading matches, the section is
created at the end of the note: give SECTION its own `#` marks to choose
the level, else it becomes a `## ` heading.")]
    Append {
        /// Note title, ISO date, period, or today/tomorrow/yesterday
        note: String,
        /// The text to append, quoted; may span lines
        text: String,
        /// Section to append into (e.g. \"Todays Tasks\")
        #[arg(long, value_name = "HEADING")]
        section: Option<String>,
    },

    /// Change or delete one line of a note
    #[command(long_about = "\
Change exactly one line of a note. --find gives text the line contains
(case-insensitive); when several lines match, nothing is changed, the
matches are listed, and the exit code is 4 — rerun with more of the
line's words. The change is one of:

  --replace TEXT   the whole line becomes TEXT
  --append TEXT    TEXT joins the end of the line (separated by a space
                   unless TEXT starts with whitespace)
  --delete         the line is removed

The edit applies against the note as it is on disk right now; if the
matched line changed underneath the edit, nothing is written and the
command fails so it can be rerun.")]
    Edit {
        /// Note title, ISO date, period, or today/tomorrow/yesterday
        note: String,
        /// Text the target line contains, quoted
        #[arg(long, value_name = "TEXT")]
        find: String,
        /// The whole new line
        #[arg(long, value_name = "TEXT", conflicts_with_all = ["append", "delete"])]
        replace: Option<String>,
        /// Text to add at the end of the line
        #[arg(long, value_name = "TEXT", conflicts_with = "delete")]
        append: Option<String>,
        /// Remove the line
        #[arg(long, action = ArgAction::SetTrue)]
        delete: bool,
    },

    /// Move overdue tasks from past days into today
    #[command(long_about = "\
Move every overdue open task off past daily notes and into one day
(today, or --to). Each task line is appended to the destination and
deleted from its old day — never duplicated — with its stale `>date`
token stripped, since the day it lands on dates it. A `**group**` header
travels with its tasks: written once at the destination, deleted from
the old day when nothing is left under it.

Left alone: tasks due today or later (their `>date` token already says
when), scheduled ([>]) and cancelled ([-]) tasks, and overdue tasks
living in regular notes (their home is a note, not a day). --dry-run
prints what would move without writing anything. Nothing to carry is
success: a clean morning prints one line and exits 0.")]
    Carry {
        /// Day the tasks land on: today, tomorrow, or 2026-08-12
        #[arg(long, value_name = "WHEN", default_value = "today")]
        to: String,
        /// Section of the destination note to append into
        #[arg(long, value_name = "HEADING")]
        section: Option<String>,
        /// Print the plan without writing
        #[arg(long, action = ArgAction::SetTrue)]
        dry_run: bool,
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

    /// List recently modified notes
    #[command(long_about = "\
List notes whose file changed in the last N days (default 3), newest
first, one per line as `MODIFIED  FILE`. Daily, period, and regular notes
all count. Modification time is what matters, not the note's date: a
last-week daily edited yesterday appears, which is how backfilled notes
get noticed.")]
    Recent {
        /// How many days back to look
        #[arg(long, value_name = "N", default_value_t = 3)]
        days: u64,
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
        Command::Tasks { today, overdue, done, since } => {
            let query = match (today, overdue) {
                (true, _) => TaskQuery::Today,
                (_, true) => TaskQuery::Overdue,
                _ => TaskQuery::Open,
            };
            let since = since.as_deref().map(parse_when).transpose()?;
            cmd_tasks(&root, query, done, since, cli.json)
        }
        Command::Add { text, date, section } => {
            let date = parse_when(&date)?;
            cmd_add_task(&root, date, &text, section.as_deref(), "add", &actor, cli.json)
        }
        Command::Capture { text } => {
            let date = Local::now().date_naive();
            cmd_add_task(&root, date, &text, None, "capture", &actor, cli.json)
        }
        Command::Done { r#match } => cmd_done(&root, &r#match, &actor, cli.json),
        Command::Append { note, text, section } => {
            cmd_append(&root, &note, &text, section.as_deref(), &actor, cli.json)
        }
        Command::Edit { note, find, replace, append, delete } => {
            let op = if delete {
                EditOp::Delete
            } else if let Some(new_line) = replace {
                EditOp::Replace(new_line)
            } else if let Some(suffix) = append {
                EditOp::Append(suffix)
            } else {
                return Err(Failure::new(2, "say what to change")
                    .hint("one of --replace TEXT, --append TEXT, or --delete"));
            };
            cmd_edit(&root, &note, &find, op, &actor, cli.json)
        }
        Command::Carry { to, section, dry_run } => {
            let dest = parse_when(&to)?;
            cmd_carry(&root, dest, section.as_deref(), dry_run, &actor, cli.json)
        }
        Command::Search { query, limit } => cmd_search(&root, &query, limit, cli.json),
        Command::Recent { days } => cmd_recent(&root, days, cli.json),
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
/// the state bracket ([ ], [x], …) stripped. What matching and display use.
fn task_text(line: &str) -> &str {
    let s = line.trim_start();
    let s = ["* ", "+ ", "- "]
        .iter()
        .find_map(|m| s.strip_prefix(m))
        .unwrap_or(s);
    let s = s.trim_start();
    let bytes = s.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'[' && bytes[1].is_ascii() && bytes[2] == b']' {
        s[3..].trim_start()
    } else {
        s
    }
}

/// Where a NOTE argument points: a day (which may not have a note yet) or
/// an existing note file.
enum Target {
    Day(NaiveDate),
    Note(PathBuf),
}

/// Resolve a NOTE argument the way `kairn note` reads titles — wiki-link
/// rules — plus the day words the --date flags accept. Missing titles fail
/// exit 3 with close-title suggestions.
fn resolve_target(root: &Path, arg: &str) -> Result<Target, Failure> {
    let today = Local::now().date_naive();
    match arg.trim().to_lowercase().as_str() {
        "today" => return Ok(Target::Day(today)),
        "tomorrow" => return Ok(Target::Day(today + Duration::days(1))),
        "yesterday" => return Ok(Target::Day(today - Duration::days(1))),
        _ => {}
    }
    match resolve_wiki_target(root, arg) {
        WikiTarget::Day(date) => Ok(Target::Day(date)),
        WikiTarget::Note(path) => Ok(Target::Note(path)),
        WikiTarget::Missing(_) => {
            let mut failure = Failure::new(EXIT_NOT_FOUND, format!("no note called {arg:?}"));
            let near: Vec<String> = search_notes(root, arg, 4)
                .into_iter()
                .filter(|hit| hit.snippet.is_none())
                .map(|hit| hit.name)
                .collect();
            if !near.is_empty() {
                failure = failure.hint(format!("close titles: {}", near.join(", ")));
            }
            Err(failure)
        }
        WikiTarget::Invalid => Err(Failure::new(2, format!("{arg:?} cannot name a note"))
            .hint("titles cannot contain empty, dot-leading, or backslash parts")),
    }
}

/// The day's existing note file, exit 3 with the way to start one when the
/// day has none.
fn existing_daily(root: &Path, date: NaiveDate) -> Result<PathBuf, Failure> {
    daily_file(root, date).ok_or_else(|| {
        Failure::new(
            EXIT_NOT_FOUND,
            format!("no daily note for {}", date.format("%Y-%m-%d")),
        )
        .hint(format!(
            "kairn add \"...\" --date {} starts one",
            date.format("%Y-%m-%d")
        ))
    })
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
    let path = match resolve_target(root, title)? {
        Target::Day(date) => existing_daily(root, date)?,
        Target::Note(path) => path,
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
        "due": task.due.format("%Y-%m-%d").to_string(),
        "file": rel(root, &task.path),
        "line": task.line_idx + 1,
        "text": task_text(&task.line),
    })
}

fn cmd_tasks(
    root: &Path,
    query: TaskQuery,
    done: bool,
    since: Option<NaiveDate>,
    json: bool,
) -> Result<(), Failure> {
    let today = Local::now().date_naive();
    let source = if done { done_tasks_in_vault(root) } else { open_tasks_in_vault(root) };
    let tasks: Vec<TaskRef> = source
        .into_iter()
        .filter(|t| query.matches(t.due, today))
        .filter(|t| since.is_none_or(|s| t.due >= s))
        .collect();
    if json {
        let body = json!({
            "count": tasks.len(),
            "tasks": tasks.iter().map(|t| task_json(root, t)).collect::<Vec<_>>(),
        });
        println!("{body}");
    } else {
        for task in &tasks {
            println!("{}  {}", task.due.format("%Y-%m-%d"), task_text(&task.line));
        }
        if tasks.is_empty() {
            let kind = if done { "done" } else { "open" };
            eprintln!("kairn: no matching {kind} tasks");
        }
    }
    Ok(())
}

fn cmd_add_task(
    root: &Path,
    date: NaiveDate,
    text: &str,
    section: Option<&str>,
    action: &str,
    actor: &str,
    json: bool,
) -> Result<(), Failure> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Failure::new(2, "nothing to add: the text is empty"));
    }
    // The app's configured daily-template rule decides whether a brand-new
    // day gets seeded, so a CLI capture matches what the app would show.
    let rule = Settings::load().daily_template_rule;
    let write_err =
        |e| Failure::new(EXIT_FAILED, format!("could not write the daily note: {e}"));
    let path = match section {
        Some(heading) => {
            let path = write::ensure_day_note(root, date, &rule).map_err(write_err)?;
            write::append_to_section(&path, heading, &format!("* {text}"))
                .map_err(write_err)?;
            path
        }
        None => write::append_to_day(root, date, text, &rule).map_err(write_err)?,
    };
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

fn cmd_append(
    root: &Path,
    note: &str,
    text: &str,
    section: Option<&str>,
    actor: &str,
    json: bool,
) -> Result<(), Failure> {
    let text = text.trim_matches('\n');
    if text.trim().is_empty() {
        return Err(Failure::new(2, "nothing to append: the text is empty"));
    }
    let path = match resolve_target(root, note)? {
        Target::Day(date) => {
            let rule = Settings::load().daily_template_rule;
            write::ensure_day_note(root, date, &rule).map_err(|e| {
                Failure::new(EXIT_FAILED, format!("could not write the daily note: {e}"))
            })?
        }
        Target::Note(path) => path,
    };
    let idx = match section {
        Some(heading) => write::append_to_section(&path, heading, text),
        None => write::append_line(&path, text),
    }
    .map_err(|e| {
        Failure::new(EXIT_FAILED, format!("could not write {}: {e}", path.display()))
    })?;
    log_write(root, actor, "append", &path, text);
    if json {
        let body = json!({
            "file": rel(root, &path),
            "line": idx + 1,
            "text": text,
        });
        println!("{body}");
    } else {
        println!("added to {}", rel(root, &path));
    }
    Ok(())
}

/// What `kairn edit` does to the one matched line.
enum EditOp {
    Replace(String),
    Append(String),
    Delete,
}

/// The matched line with `suffix` joined to its end: separated by one
/// space unless the suffix brings its own whitespace, and always before
/// trailing whitespace, which is content (markdown hard breaks).
fn append_suffix(line: &str, suffix: &str) -> String {
    let trimmed = line.trim_end();
    let trailing = &line[trimmed.len()..];
    if suffix.starts_with(char::is_whitespace) {
        format!("{trimmed}{suffix}{trailing}")
    } else {
        format!("{trimmed} {suffix}{trailing}")
    }
}

fn cmd_edit(
    root: &Path,
    note: &str,
    find: &str,
    op: EditOp,
    actor: &str,
    json: bool,
) -> Result<(), Failure> {
    let needle = find.trim();
    if needle.is_empty() {
        return Err(Failure::new(2, "give some words from the line to change"));
    }
    let path = match resolve_target(root, note)? {
        Target::Day(date) => existing_daily(root, date)?,
        Target::Note(path) => path,
    };
    let text = read_note(&path)?;
    let lower = needle.to_lowercase();
    let matches: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.to_lowercase().contains(&lower))
        .collect();
    let (idx, line) = match matches.as_slice() {
        [one] => *one,
        [] => {
            return Err(Failure::new(
                EXIT_NOT_FOUND,
                format!("no line in {} contains {needle:?}", rel(root, &path)),
            ));
        }
        several => {
            let listed: Vec<String> = several
                .iter()
                .take(6)
                .map(|(i, line)| format!("  {}: {}", i + 1, line.trim()))
                .collect();
            return Err(Failure::new(
                EXIT_AMBIGUOUS,
                format!(
                    "{} lines in {} contain {needle:?}:\n{}",
                    several.len(),
                    rel(root, &path),
                    listed.join("\n")
                ),
            )
            .hint("rerun with more of the line's words to pick one"));
        }
    };
    let line = line.to_string();
    let (action, after) = match &op {
        EditOp::Replace(new_line) => ("edit", Some(new_line.clone())),
        EditOp::Append(suffix) => ("edit", Some(append_suffix(&line, suffix))),
        EditOp::Delete => ("delete", None),
    };
    let applied = match &after {
        Some(new_line) => write::replace_line_on_disk(&path, idx, &line, new_line),
        None => write::remove_line_on_disk(&path, idx, &line),
    }
    .map_err(|e| {
        Failure::new(EXIT_FAILED, format!("could not update {}: {e}", path.display()))
    })?;
    if applied.is_none() {
        return Err(Failure::new(
            EXIT_FAILED,
            "the note changed while editing and the line could not be found again",
        )
        .hint("run the same command again"));
    }
    log_write(root, actor, action, &path, after.as_deref().unwrap_or(&line));
    if json {
        let body = json!({
            "file": rel(root, &path),
            "before": line,
            "after": after,
        });
        println!("{body}");
    } else {
        match &after {
            Some(new_line) => println!("edited: {new_line}"),
            None => println!("deleted: {line}"),
        }
    }
    Ok(())
}

fn cmd_carry(
    root: &Path,
    dest: NaiveDate,
    section: Option<&str>,
    dry_run: bool,
    actor: &str,
    json: bool,
) -> Result<(), Failure> {
    let today = Local::now().date_naive();
    if dest < today {
        return Err(Failure::new(2, "carry moves tasks forward: --to must be today or later"));
    }
    let mut candidates: Vec<TaskRef> = open_tasks_in_vault(root)
        .into_iter()
        .filter(|t| t.file_date.is_some_and(|d| d < today) && t.due < dest)
        .collect();
    // Oldest day first, file order within a day, so the destination reads
    // chronologically.
    candidates.sort_by(|a, b| {
        a.file_date.cmp(&b.file_date).then_with(|| a.line_idx.cmp(&b.line_idx))
    });
    if candidates.is_empty() {
        if json {
            let body = json!({
                "count": 0,
                "to": dest.format("%Y-%m-%d").to_string(),
                "dry_run": dry_run,
                "tasks": [],
            });
            println!("{body}");
        } else {
            eprintln!("kairn: nothing to carry");
        }
        return Ok(());
    }
    let mut texts: HashMap<PathBuf, String> = HashMap::new();
    for task in &candidates {
        if !texts.contains_key(&task.path) {
            texts.insert(task.path.clone(), read_note(&task.path)?);
        }
    }
    let moves: Vec<carry::Move> = candidates
        .into_iter()
        .map(|task| {
            let lines: Vec<&str> = texts[&task.path].lines().collect();
            let header = carry::group_header(&lines, task.line_idx.min(lines.len()));
            let carried_line = carry::strip_due_token(&task.line);
            carry::Move { task, header, carried_line }
        })
        .collect();
    let tasks_json: Vec<serde_json::Value> = moves
        .iter()
        .map(|m| {
            json!({
                "from": m.task.file_date.map(|d| d.format("%Y-%m-%d").to_string()),
                "text": task_text(&m.carried_line),
                "header": m.header.as_deref().map(task_text),
            })
        })
        .collect();
    if dry_run {
        if json {
            let body = json!({
                "count": moves.len(),
                "to": dest.format("%Y-%m-%d").to_string(),
                "dry_run": true,
                "tasks": tasks_json,
            });
            println!("{body}");
        } else {
            println!("would carry {} tasks to {}:", moves.len(), dest.format("%Y-%m-%d"));
            for m in &moves {
                let from = m.task.file_date.map_or_else(String::new, |d| {
                    d.format("%Y-%m-%d").to_string()
                });
                println!("  {from}  {}", task_text(&m.carried_line));
            }
        }
        return Ok(());
    }
    // The destination gains the block first, then the source lines go: a
    // line that changes underneath the delete ends up duplicated, never
    // lost, and the failure is reported.
    let rule = Settings::load().daily_template_rule;
    let dest_path = write::ensure_day_note(root, dest, &rule).map_err(|e| {
        Failure::new(EXIT_FAILED, format!("could not write the daily note: {e}"))
    })?;
    let block = carry::destination_block(&moves);
    match section {
        Some(heading) => {
            write::append_to_section(&dest_path, heading, &block).map(|_| ())
        }
        None => write::append_line(&dest_path, &block).map(|_| ()),
    }
    .map_err(|e| {
        Failure::new(EXIT_FAILED, format!("could not write {}: {e}", dest_path.display()))
    })?;
    let mut by_file: HashMap<&PathBuf, Vec<&carry::Move>> = HashMap::new();
    for m in &moves {
        by_file.entry(&m.task.path).or_default().push(m);
    }
    let mut leftovers: Vec<String> = Vec::new();
    for (path, mut file_moves) in by_file {
        // Bottom-up so earlier removals never shift a later line's index.
        file_moves.sort_by_key(|m| std::cmp::Reverse(m.task.line_idx));
        for m in &file_moves {
            match write::remove_line_on_disk(path, m.task.line_idx, &m.task.line) {
                Ok(Some(_)) => {}
                Ok(None) => leftovers
                    .push(format!("{}: {}", rel(root, path), task_text(&m.task.line))),
                Err(e) => {
                    return Err(Failure::new(
                        EXIT_FAILED,
                        format!("could not update {}: {e}", path.display()),
                    ));
                }
            }
        }
        let headers: Vec<&str> = {
            let mut seen = Vec::new();
            for m in &file_moves {
                if let Some(h) = m.header.as_deref()
                    && !seen.contains(&h)
                {
                    seen.push(h);
                }
            }
            seen
        };
        if headers.is_empty() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let emptied: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(i, line)| {
                headers.contains(line) && carry::header_now_empty(&lines, *i)
            })
            .map(|(i, _)| i)
            .collect();
        for i in emptied.iter().rev() {
            if let Err(e) = write::remove_line_on_disk(path, *i, lines[*i]) {
                eprintln!("kairn: could not remove an emptied group header: {e}");
            }
        }
    }
    for m in &moves {
        log_write(root, actor, "carry", &dest_path, task_text(&m.carried_line));
    }
    for line in &leftovers {
        eprintln!(
            "kairn: carried but not removed (the line changed underneath): {line} — now in both notes, remove the old one by hand"
        );
    }
    if json {
        let body = json!({
            "count": moves.len(),
            "to": dest.format("%Y-%m-%d").to_string(),
            "file": rel(root, &dest_path),
            "dry_run": false,
            "tasks": tasks_json,
            "left_in_place": leftovers,
        });
        println!("{body}");
    } else {
        println!("carried {} tasks to {}", moves.len(), dest.format("%Y-%m-%d"));
        for m in &moves {
            let from = m.task.file_date.map_or_else(String::new, |d| {
                d.format("%Y-%m-%d").to_string()
            });
            println!("  {from}  {}", task_text(&m.carried_line));
        }
    }
    Ok(())
}

fn cmd_recent(root: &Path, days: u64, json: bool) -> Result<(), Failure> {
    use std::time::{Duration as StdDuration, SystemTime};
    let scan = vault::VaultScan::new(root);
    let cutoff = SystemTime::now()
        .checked_sub(StdDuration::from_secs(days.saturating_mul(86400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut entries: Vec<(SystemTime, PathBuf)> = Vec::new();
    {
        let mut consider = |path: &PathBuf| {
            if let Ok(meta) = std::fs::metadata(path)
                && let Ok(modified) = meta.modified()
                && modified >= cutoff
            {
                entries.push((modified, path.clone()));
            }
        };
        for path in scan.days.values() {
            consider(path);
        }
        for (_, path) in &scan.periods {
            consider(path);
        }
        for path in scan.note_files() {
            consider(path);
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    if json {
        let body = json!({
            "count": entries.len(),
            "notes": entries.iter().map(|(modified, path)| {
                let local: chrono::DateTime<Local> = (*modified).into();
                json!({
                    "file": rel(root, path),
                    "modified": local.format("%Y-%m-%d %H:%M:%S").to_string(),
                })
            }).collect::<Vec<_>>(),
        });
        println!("{body}");
    } else {
        for (modified, path) in &entries {
            let local: chrono::DateTime<Local> = (*modified).into();
            println!("{}  {}", local.format("%Y-%m-%d %H:%M"), rel(root, path));
        }
        if entries.is_empty() {
            eprintln!("kairn: nothing modified in the last {days} days");
        }
    }
    Ok(())
}

fn cmd_done(root: &Path, needle: &str, actor: &str, json: bool) -> Result<(), Failure> {
    let needle_trim = needle.trim();
    if needle_trim.is_empty() {
        return Err(Failure::new(2, "give some words from the task's text"));
    }
    let tasks = open_tasks_in_vault(root);
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
            scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
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
                .map(|t| format!("  {}  {}", t.due.format("%Y-%m-%d"), task_text(&t.line)))
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
            "due": task.due.format("%Y-%m-%d").to_string(),
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
    fn task_text_strips_markers_and_state_brackets() {
        assert_eq!(task_text("* buy milk"), "buy milk");
        assert_eq!(task_text("  + [ ] pack bag"), "pack bag");
        assert_eq!(task_text("- [ ]  call bank"), "call bank");
        assert_eq!(task_text("* [x] shipped"), "shipped");
        assert_eq!(task_text("* [-] dropped"), "dropped");
        // Content brackets past the marker stay content.
        assert_eq!(task_text("* see [[note]]"), "see [[note]]");
    }

    #[test]
    fn suffixes_join_with_one_space_before_trailing_whitespace() {
        assert_eq!(append_suffix("* call Sam", "→ GDL-42"), "* call Sam → GDL-42");
        // A suffix bringing its own whitespace is taken verbatim.
        assert_eq!(append_suffix("* call Sam", "  (twice)"), "* call Sam  (twice)");
        // Trailing whitespace is content (markdown hard breaks): the suffix
        // lands before it.
        assert_eq!(append_suffix("* call Sam  ", "→ GDL-42"), "* call Sam → GDL-42  ");
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
