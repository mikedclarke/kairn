# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

- Weekly (2026-W33) and monthly (2026-08) notes join dailies, with the sidebar calendar switching between a day view, a week picker, and a months grid.
- A clock tab on the calendar switcher opens a day timeline in the sidebar, where timed tasks drag to a new time, resize to change length, and drop onto a calendar day to move there.
- Select several tasks and drag any handle inside the selection to move them all at once: within the note, onto a calendar day, or into a section.
- Undo covers moves: Cmd+Z (Ctrl+Shift+Z on Linux) takes back a drag to another day, an in-note reorder, or a timeline retime, and Cmd+Shift+Z (Ctrl+Shift+Y on Linux) re-applies it.
- Typed clock times and time ranges highlight like links in the editor.
- Themes are picked from one searchable dropdown: a new Menlo theme is the default, and the old Default look becomes the Sage and Sage Light themes.
- Every layout has a direct keyboard shortcut in switcher order: Cmd+Option+1 to 4 on macOS, Ctrl+Alt+1 to 4 on Linux.
- Titlebar controls show a hover tooltip naming the control and its keyboard shortcut.

## Improved

- The Daily, Weekly, and Monthly switcher is a slim strip under the calendar, with the timeline clock tab first and the active mode outlined as a tab with cut corners.
- Settings is a full page instead of a popup: sections in a left rail, one scrolling column, one stable size; Esc or Back returns to the app and applies your changes.
- The titlebar search and layout switcher are one capsule, and the search hint names what search covers: notes, days, and sessions.
- The month picker fills the same height as the day calendar, so switching views no longer shifts the sidebar.
- New installs start with the week strip shown on daily notes and the Tasks and Agents sections hidden.
- The timeline pill row above daily notes and the daily-list direction setting are retired.

## Fixed

- Calendar switcher and month picker labels no longer shift while hovered.
- Week strip day cards no longer overflow the strip at larger interface sizes.
