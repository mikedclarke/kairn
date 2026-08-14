# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

A clock tab on the calendar switcher opens a day timeline in the sidebar: the day's timed tasks appear as blocks that drag to a new time, resize from the bottom edge to change length, and drop onto a calendar day to move there.

Every layout has a direct keyboard shortcut in switcher order: Cmd+Option+1 to 4 on macOS, Ctrl+Alt+1 to 4 on Linux.

Titlebar controls show a hover tooltip naming the control with its keyboard shortcut beneath, so shortcuts are learnable from the UI.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

The Daily, Weekly, and Monthly switcher is now a slim strip drawn as a single line under the calendar, with the active mode outlined as an upside-down tab with 45-degree cut corners.

Settings is a full page instead of a popup: sections in a left rail, one scrolling content column, one stable size, and room to grow; Esc or Back returns to the app and applies your changes.

The timeline clock tab sits first on the calendar switcher, next to Daily, so the two swap with one small pointer move.

The titlebar search and layout switcher are one capsule at a single height, with all four layouts (Writing, Notes, Notes + Terminal, Terminal) as matching drawn icons.

The titlebar search hint says what search covers (notes, days, sessions) instead of just jump.

The calendar starts slightly higher in the sidebar.

The month picker fills the same height as the day calendar, so switching views no longer shifts the sidebar.

The timeline pill row above daily notes and its setting are retired in favour of the sidebar timeline.

## Fixed

<!-- Bugs that no longer happen. -->

Calendar switcher and month picker labels no longer shift left while hovered.

Week strip day cards no longer overflow the strip at larger interface sizes.
