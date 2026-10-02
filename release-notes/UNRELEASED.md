# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md and the GitHub release, which serves as the public changelog):
one short sentence per bullet stating the change; no usage tips, no background, no
restating the same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

- `kairn section` replaces, creates, or deletes one heading's block in a note as a unit, writing nothing when the block is already up to date, so scripts and agents can keep a status block current and logged in the activity sidebar.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

## Fixed

<!-- Bugs that no longer happen. -->

- Deferred sync pulls persist across restarts and keep sync in an error state until applied or superseded.
- Sync stops when its vault disappears or is replaced, before a pull can recreate it or push deletions.
- Sync preserves conflict copies before replacing notes and reuses them when a failed sync is retried.
- App, CLI and sync writes share a lock across reading and replacement; busy files refuse the write for retry.

- The process-only notes folder override remains active after closing Settings.
- Library text and code saves merge external changes and preserve conflicting edits as separate copies.
- Failed saves preserve unsaved text in a local recovery folder and display its location.
- Undoing a moved block after an external edit and handling Unicode text no longer cause note operations to panic.
- Conflicting note edits are saved as recovery copies that survive closing the app, and simultaneous line insertions keep both additions.
