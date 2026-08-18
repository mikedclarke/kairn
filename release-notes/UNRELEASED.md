# Unreleased

Draft notes for the next Kairn release.

Keep this user-facing. Commits are the complete raw ledger; this file is for changes that
should be remembered when the next release is curated. Copy rules (these notes publish
verbatim to CHANGELOG.md and the GitHub release, which serves as the public changelog):
one short sentence per bullet stating the change; no usage tips, no background, no
restating the same fact twice; no em dashes. Rewrite to this standard before releasing.

## Added

- Pasting a bare web link fetches the page title and turns it into a markdown link.

## Improved

- Regular notes open as just the document: the pane title, folder line, and divider are gone, and the note's own first heading is the title.
- Sessions live in the titlebar: a small indicator shows how many are open, and clicking it switches, closes, or starts sessions; the sidebar section is retired.
- The week strip lines up with the note's centred width in the Writing layout instead of stretching across the window.
- Dash lists show a round bullet dot instead of a grey dash.

## Fixed

- Wrapped lines no longer strand a closing bracket or trailing punctuation at the start of the next line.
- Enter at the start of a heading, quote, or indented line moves the whole line down instead of splitting off its markdown prefix.
