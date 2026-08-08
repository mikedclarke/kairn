//! Vault layout conventions the phone needs to resolve files the same way the
//! desktop does. The naming rules live in [`kairn_core::vault`]; this reuses
//! them (rather than re-encoding the format string) so there is no drift.

use std::path::Path;

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
    fn invalid_date_is_none() {
        assert!(daily_note_path(2026, 2, 30).is_none());
    }
}
