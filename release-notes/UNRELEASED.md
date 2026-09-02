# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md and the GitHub release, which serves as the public changelog):
one short sentence per bullet stating the change; no usage tips, no background, no
restating the same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

- On Linux, an idle window no longer wakes the app and the compositor 60 times a second, cutting idle CPU, GPU, and battery use.
- An unchanged note is no longer re-parsed and re-shaped on every redraw.
- The caret stops blinking after ten seconds without input and while the window is in the background.
- Terminal output repaints at most 30 times a second, and 10 times a second in a pane that is not focused.

## Fixed

<!-- Bugs that no longer happen. -->

- `kairn carry` now moves italic task-group headers (`*Group*`, `_Group_`) with their tasks, not only `**bold**` ones, and removes them from the old day when emptied.
