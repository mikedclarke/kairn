//! Vault layout conventions the phone needs to resolve files the same way the
//! desktop does: period note naming, search, conflict copies, and the daily
//! template. The rules live in [`kairn_core::vault`] and
//! [`kairn_core::template`]; this reuses them (rather than re-encoding format
//! strings and walk orders) so there is no drift.

use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};

/// A calendar date crossing the FFI, avoiding stringly-typed dates.
#[derive(Clone, Copy, uniffi::Record)]
pub struct FfiDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl FfiDate {
    pub(crate) fn to_naive(self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
    }
}

impl From<NaiveDate> for FfiDate {
    fn from(d: NaiveDate) -> Self {
        Self { year: d.year(), month: d.month(), day: d.day() }
    }
}

fn path_string(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

/// The vault-relative path of a date's daily note, e.g. `Calendar/20260808.md`.
/// The absolute path is this joined onto the device's vault root, which the
/// Swift side owns. Returns `None` for an invalid date.
///
/// Reuses [`kairn_core::daily_path`] against an empty root so the naming stays
/// byte-for-byte identical to what the desktop writes.
#[uniffi::export]
pub fn daily_note_path(year: i32, month: u32, day: u32) -> Option<String> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let rel = kairn_core::daily_path(Path::new(""), date);
    Some(rel.to_string_lossy().into_owned())
}

/// The vault-relative path of the date's ISO-week weekly note, e.g.
/// `Calendar/2026-W33.md` (the week-year, which differs from the calendar
/// year around New Year). `None` for an invalid date.
#[uniffi::export]
pub fn weekly_note_path(year: i32, month: u32, day: u32) -> Option<String> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    Some(path_string(kairn_core::weekly_path(Path::new(""), date)))
}

/// The vault-relative path of the date's monthly note, e.g.
/// `Calendar/2026-08.md`. `None` for an invalid date.
#[uniffi::export]
pub fn monthly_note_path(year: i32, month: u32, day: u32) -> Option<String> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    Some(path_string(kairn_core::monthly_path(Path::new(""), date)))
}

/// One search result. `path` is absolute (under the `root` the query ran
/// against); `date` is set when the hit is a daily note; `snippet` is the
/// matching body line when the match wasn't in the title.
#[derive(uniffi::Record)]
pub struct FfiSearchHit {
    pub path: String,
    pub date: Option<FfiDate>,
    pub name: String,
    pub snippet: Option<String>,
}

/// Search every note under `root`: fuzzy title matches first (best score
/// wins), then one substring body match per file, dailies newest first,
/// capped at `limit`. Mirrors the desktop switcher's search exactly.
#[uniffi::export]
pub fn search_notes(root: String, query: String, limit: u32) -> Vec<FfiSearchHit> {
    kairn_core::search_notes(Path::new(&root), &query, limit as usize)
        .into_iter()
        .map(|hit| FfiSearchHit {
            path: path_string(hit.path),
            date: hit.date.map(Into::into),
            name: hit.name,
            snippet: hit.snippet,
        })
        .collect()
}

/// A date-shaped query (`2026-08-12`, `aug 12`, `12 Aug`, `tomorrow`…)
/// resolved to the day it names, relative to `today`. `None` when the query
/// doesn't read as a date. Mirrors the desktop switcher's day jump.
#[uniffi::export]
pub fn parse_day_query(query: String, today: FfiDate) -> Option<FfiDate> {
    let today = today.to_naive()?;
    kairn_core::parse_day_query(&query, today).map(Into::into)
}

/// A sync-conflict copy and the note it belongs to, both absolute paths.
#[derive(uniffi::Record)]
pub struct FfiConflict {
    pub owner: String,
    pub copy: String,
}

/// Every sync-conflict copy under the vault (`Calendar/` plus the whole
/// `Notes/` tree), paired with the note it shadows.
#[uniffi::export]
pub fn vault_conflicts(root: String) -> Vec<FfiConflict> {
    kairn_core::vault_conflicts(Path::new(&root))
        .into_iter()
        .map(|(owner, copy)| FfiConflict {
            owner: path_string(owner),
            copy: path_string(copy),
        })
        .collect()
}

/// The sync-conflict copies sitting next to `path`, oldest first.
#[uniffi::export]
pub fn conflict_copies(path: String) -> Vec<String> {
    kairn_core::conflict_copies(Path::new(&path))
        .into_iter()
        .map(path_string)
        .collect()
}

/// The daily template body from `Notes/@Templates/Daily.md` (frontmatter
/// stripped), or `None` when no usable template exists. The date rule is the
/// caller's to apply via [`template_applies`].
#[uniffi::export]
pub fn daily_template(root: String) -> Option<String> {
    kairn_core::daily_template(Path::new(&root))
}

/// Whether the daily template applies on `date` under `rule` (`always`,
/// `weekdays`, or `off`). Mirrors the desktop's template rule.
#[uniffi::export]
pub fn template_applies(rule: String, date: FfiDate) -> bool {
    let Some(date) = date.to_naive() else { return false };
    kairn_core::template_applies(&rule, date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_note_path_matches_desktop_naming() {
        assert_eq!(
            daily_note_path(2026, 8, 8).unwrap(),
            "Calendar/20260808.md"
        );
    }

    #[test]
    fn period_paths_match_desktop_naming() {
        assert_eq!(weekly_note_path(2026, 8, 15).unwrap(), "Calendar/2026-W33.md");
        assert_eq!(monthly_note_path(2026, 8, 15).unwrap(), "Calendar/2026-08.md");
        // ISO week-year: 1 Jan 2027 belongs to 2026's week 53.
        assert_eq!(weekly_note_path(2027, 1, 1).unwrap(), "Calendar/2026-W53.md");
    }

    #[test]
    fn invalid_date_is_none() {
        assert!(daily_note_path(2026, 2, 30).is_none());
        assert!(weekly_note_path(2026, 2, 30).is_none());
        assert!(monthly_note_path(2026, 2, 30).is_none());
    }

    #[test]
    fn day_query_resolves_relative_to_today() {
        let today = FfiDate { year: 2026, month: 8, day: 15 };
        let hit = parse_day_query("aug 12".into(), today).unwrap();
        assert_eq!((hit.year, hit.month, hit.day), (2026, 8, 12));
        assert!(parse_day_query("not a date".into(), today).is_none());
    }

    #[test]
    fn template_rule_weekdays() {
        // 2026-08-15 is a Saturday.
        let sat = FfiDate { year: 2026, month: 8, day: 15 };
        let mon = FfiDate { year: 2026, month: 8, day: 17 };
        assert!(!template_applies("weekdays".into(), sat));
        assert!(template_applies("weekdays".into(), mon));
        assert!(template_applies("always".into(), sat));
        assert!(!template_applies("off".into(), mon));
    }
}
