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
- Headings and bold text now carry their own theme colours instead of rendering plain white.
- Three colour presets, Ocean, Rose, and Forest, join Dark and Light in the theme picker.
- Interface text size is adjustable in Settings, separately from the editor text size.
- Clicking the month and year above the calendar jumps back to today and opens today's note.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

- New notes open untitled with the cursor on the title line, and the file is renamed to match the title as it is written.
- The week strip now shows the same tick and ring day indicators as the calendar.
- The calendar and week-strip arrows and the folder triangles are larger and easier to see, and calendar day numbers are slightly larger.
- The font pickers offer a curated set of installed families instead of every font on the system.
- The text selection highlight is clearer and no longer dims the selected text.
- Completing a task no longer writes an @done(date) stamp, since the note's day already dates it.

## Fixed

<!-- Bugs that no longer happen. -->

- Dialog text fields (rename note, new note) take keyboard focus when the dialog opens.
- A squeezed dialog layout can no longer scroll a text field's content out of view, which made the notes folder field look empty or mangled.
- The daily template now seeds upcoming days even when an empty daily note file already exists.
- Daily notes no longer carry a duplicate title line, since the date header already titles the day.
