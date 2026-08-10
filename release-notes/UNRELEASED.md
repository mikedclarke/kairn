# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

## Fixed

<!-- Bugs that no longer happen. -->

- On macOS the window close button now minimises Kairn to the Dock instead of leaving a stuck app that could only be force quit.
- Quick capture into a new day now follows the daily template rule, so days the template is set to skip (like weekends on the weekdays setting) stay plain.
- Typing into a day whose file already existed empty no longer wipes the note into a conflict banner.
- Editing an existing note no longer renames its file to match its first heading; only new untitled notes take their name from the typed title, when you save or leave the note.
- A new note whose title clashes with an existing note now shows a notice instead of silently keeping the Untitled name.
