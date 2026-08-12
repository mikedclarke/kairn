# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

- Cmd+click (Ctrl+click on Linux) opens links in terminal output, covering OSC 8 hyperlinks and plain URLs, including URLs that wrap across lines.
- Holding Cmd underlines the terminal link under the pointer.
- Sync conflicts can now be resolved from the conflict banner: keep this version or use the copy, with the losing file moved to the vault trash.
- The conflict banner shows the copy's filename and a Copy path action.
- A sidebar section lists every sync conflict in the vault, so conflicts on notes that aren't open no longer go unnoticed.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

## Fixed

<!-- Bugs that no longer happen. -->

- Hover drag handles now center vertically on their line instead of sitting too high, on both platforms.
