# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

- The notes folder is chosen with the system folder picker instead of typing a path.
- A KAIRN_ROOT environment variable points the app at a different notes folder for one session.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

## Fixed

<!-- Bugs that no longer happen. -->

- Dialog text fields (rename note, new note) take keyboard focus when the dialog opens.
- A squeezed dialog layout can no longer scroll a text field's content out of view, which made the notes folder field look empty or mangled.
