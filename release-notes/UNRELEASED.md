# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

- Launch shortcuts: named commands saved in Settings for this machine or any SSH host, each opening in its own session.
- A start page listing shells, hosts and shortcuts replaces the shell that auto-started on launch.
- The Notes and Sessions sidebar headers gained plus buttons: add a note or folder at the top level, or start a session from the quick list.
- Folders can now be created in the notes list.
- The sidebar Agents section can be hidden in Settings.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

- Settings moved from the sidebar footer to a floating gear in the window's bottom left corner.
- The calendar's month and year heading is larger.
- The Ocean theme now uses neutral dark grey and black backgrounds instead of the olive tinted ones.

## Fixed

<!-- Bugs that no longer happen. -->

- On macOS the window close button now minimises Kairn to the Dock instead of leaving a stuck app that could only be force quit.
- Quick capture into a new day now follows the daily template rule, so days the template is set to skip (like weekends on the weekdays setting) stay plain.
- Typing into a day whose file already existed empty no longer wipes the note into a conflict banner.
- Editing an existing note no longer renames its file to match its first heading; only new untitled notes take their name from the typed title, when you save or leave the note.
- A new note whose title clashes with an existing note now shows a notice instead of silently keeping the Untitled name.
- The CLI's done command help no longer claims completed tasks get an @done timestamp.
