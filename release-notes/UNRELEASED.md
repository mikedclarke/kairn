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

- Conflicting note edits are saved as recovery copies that survive closing the app, and simultaneous line insertions keep both additions.
