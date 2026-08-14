# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md, the GitHub release, and kairnai.com/changelog): one short
sentence per bullet stating the change; no usage tips, no background, no restating the
same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

<!-- New user-visible capabilities. -->

- A Library section in the sidebar browses folders from anywhere on disk, added and removed per machine.
- Library files open by kind: markdown in the full editor, text and code in a monospace editor with autosave, images inline with a gallery strip, and everything else as a details card.
- The switcher searches vault notes and library files together.
- Every library file offers open in default app, reveal, and copy path actions, with open in browser for HTML.
- Library folders refresh live as files change on disk.
- The terminal supports the kitty keyboard protocol, so modified keys like Shift+Enter reach applications as distinct keys.
- Cmd+click (Ctrl+click on Linux) opens links in terminal output, covering OSC 8 hyperlinks and plain URLs, including URLs that wrap across lines.
- Holding Cmd underlines the terminal link under the pointer.
- Sync conflicts can now be resolved from the conflict banner: keep this version or use the copy, with the losing file moved to the vault trash.
- The conflict banner shows the copy's filename and a Copy path action.
- A sidebar section lists every sync conflict in the vault, so conflicts on notes that aren't open no longer go unnoticed.

## Improved

<!-- Refinements to existing workflows, reliability, performance, or UX. -->

- The terminal cursor takes the shape the application requests: beam, underline, hollow block, or hidden.
- Sidebar folders and files show file-type icons, with glyphs that scale with the interface size setting.
- Library files sort newest first by default, switchable to alphabetical in Settings.
- Sidebar section titles are smaller and lighter, and the list scrolls clear of the settings gear.
- Sidebar scrolling has flick momentum on Linux.

## Fixed

<!-- Bugs that no longer happen. -->

- Tab and Shift+Tab now reach terminal applications instead of being taken by window focus cycling.
- Shift, Alt, and Ctrl on arrow, navigation, and function keys now reach terminal applications.
- Copies made by terminal applications, such as tmux and vim yanks over SSH, now land on the system clipboard.
- Fonts configured in a theme resolve to a real installed family, fixing slow text rendering when a configured font was missing.
- Library file activity no longer triggers full vault reloads, which could peg a CPU core on Linux.
- Hover drag handles now center vertically on their line instead of sitting too high, on both platforms.
