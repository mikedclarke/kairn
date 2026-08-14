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

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

The Daily, Weekly, and Monthly switcher is now a slim strip drawn as a single line under the calendar, with the active mode outlined as an upside-down tab with 45-degree cut corners.

The month picker fills the same height as the day calendar, so switching views no longer shifts the sidebar.

The timeline pill row above daily notes and its setting are retired in favour of the sidebar timeline.

## Fixed

<!-- Bugs that no longer happen. -->

Calendar switcher and month picker labels no longer shift left while hovered.

Week strip day cards no longer overflow the strip at larger interface sizes.
