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
- The sidebar Daily, Tasks and Agents sections can each be hidden in Settings.
- Word, line, and note start/end cursor movement on the platform's standard keys, with shift selecting.
- Option+Backspace deletes the previous word and Cmd+Backspace deletes to the start of the line.
- Backspace at the start of a task, bullet, or quote removes the whole marker in one press.
- kairn append adds a line to any note, optionally inside a named section, creating the section when it is missing.
- kairn edit changes, extends, or deletes a single matched line of a note, refusing ambiguous matches.
- kairn carry moves overdue open tasks from past daily notes into today, group headers travelling with their tasks; it looks back 14 days by default, reports anything older, and --from widens the window.
- kairn recent lists notes by file modification time, so backfilled edits surface.
- kairn tasks can list completed tasks with --done and filter by due date with --since.
- kairn add can place a task inside a named section, and note arguments accept today, tomorrow, and yesterday.
- Every line in the editor shows a drag handle on hover; dragging reorders within the note, and indented sub-lines travel with their parent line.
- Dragging a line onto a week strip day, a calendar day, or a sidebar Daily row moves it to the top of that day's note.
- Holding a drag over a day for a second opens that day's headings, and releasing on one files the line at the end of that section.
- Escape cancels an in-flight line drag.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

- Settings moved from the sidebar footer to a floating gear in the window's bottom left corner.
- The calendar's month and year heading is larger.
- The Ocean theme now uses neutral dark grey and black backgrounds instead of the olive tinted ones.
- Checkboxes, bullets, and quote bars stay rendered on the line being edited, so a checkbox can always be ticked and text no longer shifts when the cursor arrives.
- Pressing Enter on a task continues with a rendered checkbox instead of raw markdown.
- Completed tasks keep their strikethrough while being edited.
- Task checkboxes and bullet glyphs scale with the editor text size setting.
- Clicking the glyph of a bullet or a scheduled or cancelled task places the cursor instead of doing nothing.
- Calendar day indicators sit closer under their date numbers and the rows are tighter.

## Fixed

<!-- Bugs that no longer happen. -->

- Pressing Enter on an empty line moves down a full line; blank lines no longer collapse when the cursor leaves them.

- On macOS the window close button now minimises Kairn to the Dock instead of leaving a stuck app that could only be force quit.
- Quick capture into a new day now follows the daily template rule, so days the template is set to skip (like weekends on the weekdays setting) stay plain.
- Typing into a day whose file already existed empty no longer wipes the note into a conflict banner.
- Editing an existing note no longer renames its file to match its first heading; only new untitled notes take their name from the typed title, when you save or leave the note.
- A new note whose title clashes with an existing note now shows a notice instead of silently keeping the Untitled name.
- The CLI's done command help no longer claims completed tasks get an @done timestamp.
