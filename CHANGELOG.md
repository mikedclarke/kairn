# Changelog

All notable user-facing changes to Kairn are recorded here, newest first. Changes land
in [release-notes/UNRELEASED.md](release-notes/UNRELEASED.md) as they are built and move
here when a version ships. The release process is described in
[docs/RELEASING.md](docs/RELEASING.md).

## 0.3.5 (2026-08-26)

### Improved

- Library files and folders can now be created, renamed, and deleted from the sidebar's right-click menus, with deletes going to the system trash.
- New markdown files open with a title heading instead of a blank pane.
- Symlinked folders browse, edit, and search like real folders, marked with a link icon.
- Edits made on another device now appear live instead of only after reopening the app.
- On Linux, letter shortcuts use Super as the primary modifier, with Ctrl+Shift as a fallback.

### Fixed

- On Linux, copy, paste, cut, and select-all work in the note editor under compositors such as Omarchy.

## 0.3.1 (2026-08-19)

### Added

- Pasting a bare web link fetches the page title and turns it into a markdown link.

### Improved

- Regular notes open as just the document, with the note's first heading as its title.
- Terminal sessions move to the titlebar, replacing the sidebar section.
- The week strip lines up with the note's centred width in the Writing layout instead of stretching across the window.
- Dash lists show a round bullet dot instead of a grey dash.

### Fixed

- Wrapped lines no longer strand a closing bracket or trailing punctuation at the start of the next line.
- Enter at the start of a heading, quote, or indented line moves the whole line down instead of splitting off its markdown prefix.

## 0.3.0 (2026-08-17)

### Added

- Weekly (2026-W33) and monthly (2026-08) notes join dailies, with the sidebar calendar switching between a day view, a week picker, and a months grid.
- A clock tab on the calendar switcher opens a day timeline in the sidebar, where timed tasks drag to a new time, resize to change length, and drop onto a calendar day to move there.
- Select several tasks and drag any handle inside the selection to move them all at once: within the note, onto a calendar day, or into a section.
- Undo covers moves: Cmd+Z (Ctrl+Shift+Z on Linux) takes back a drag to another day, an in-note reorder, or a timeline retime, and Cmd+Shift+Z (Ctrl+Shift+Y on Linux) re-applies it.
- Typed clock times and time ranges highlight like links in the editor.
- Themes are picked from one searchable dropdown: a new Menlo theme is the default, and the old Default look becomes the Sage and Sage Light themes.
- Every layout has a direct keyboard shortcut in switcher order: Cmd+Option+1 to 4 on macOS, Ctrl+Alt+1 to 4 on Linux.
- Titlebar controls show a hover tooltip naming the control and its keyboard shortcut.

### Improved

- The Daily, Weekly, and Monthly switcher is a slim strip under the calendar, with the timeline clock tab first and the active mode outlined as a tab with cut corners.
- Settings is a full page instead of a popup: sections in a left rail, one scrolling column, one stable size; Esc or Back returns to the app and applies your changes.
- The titlebar search and layout switcher are one capsule, and the search hint names what search covers: notes, days, and sessions.
- The month picker fills the same height as the day calendar, so switching views no longer shifts the sidebar.
- New installs start with the week strip shown on daily notes and the Tasks and Agents sections hidden.
- The timeline pill row above daily notes and the daily-list direction setting are retired.

### Fixed

- Calendar switcher and month picker labels no longer shift while hovered.
- Week strip day cards no longer overflow the strip at larger interface sizes.

## 0.2.7 (2026-08-14)

### Added

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

### Improved

- The terminal cursor takes the shape the application requests: beam, underline, hollow block, or hidden.
- Sidebar folders and files show file-type icons, with glyphs that scale with the interface size setting.
- Library files sort newest first by default, switchable to alphabetical in Settings.
- Sidebar section titles are smaller and lighter, and the list scrolls clear of the settings gear.
- Sidebar scrolling has flick momentum on Linux.

### Fixed

- Tab and Shift+Tab now reach terminal applications instead of being taken by window focus cycling.
- Shift, Alt, and Ctrl on arrow, navigation, and function keys now reach terminal applications.
- Copies made by terminal applications, such as tmux and vim yanks over SSH, now land on the system clipboard.
- Fonts configured in a theme resolve to a real installed family, fixing slow text rendering when a configured font was missing.
- Library file activity no longer triggers full vault reloads, which could peg a CPU core on Linux.
- Hover drag handles now center vertically on their line instead of sitting too high, on both platforms.

## 0.2.5 (2026-08-12)

### Added

- Launch shortcuts: named commands saved in Settings for this machine or any SSH host, each opening in its own session.
- A start page listing shells, hosts and shortcuts replaces the shell that auto-started on launch.
- The Notes and Sessions sidebar headers gained plus buttons: add a note or folder at the top level, or start a session from the quick list.
- The sidebar Daily, Tasks and Agents sections can each be hidden in Settings.
- Word, line, and note start/end cursor movement on the platform's standard keys, with shift selecting.
- Option+Backspace deletes the previous word and Cmd+Backspace deletes to the start of the line.
- Backspace at the start of a task, bullet, or quote removes the whole marker in one press.
- kairn append adds a line to any note, optionally inside a named section, creating the section when it is missing.
- kairn edit changes, extends, or deletes a single matched line of a note, refusing ambiguous matches.
- kairn carry moves overdue open tasks from past daily notes into today, keeping their group headers; it looks back 14 days by default and --from widens the window.
- kairn recent lists notes by file modification time, so backfilled edits surface.
- kairn tasks can list completed tasks with --done and filter by due date with --since.
- kairn add can place a task inside a named section, and note arguments accept today, tomorrow, and yesterday.
- Every line in the editor shows a drag handle on hover; dragging reorders within the note, and indented sub-lines travel with their parent line.
- Dragging a line onto a week strip day, a calendar day, or a sidebar Daily row moves it to the top of that day's note.
- Holding a drag over a day for a second opens that day's headings, and releasing on one files the line at the end of that section.
- Escape cancels an in-flight line drag.

### Improved

- Settings moved from the sidebar footer to a floating gear in the window's bottom left corner.
- The calendar's month and year heading is larger.
- The Ocean theme now uses neutral dark grey and black backgrounds instead of the olive tinted ones.
- Checkboxes, bullets, and quote bars stay rendered on the line being edited, so a checkbox can always be ticked and text no longer shifts when the cursor arrives.
- Pressing Enter on a task continues with a rendered checkbox instead of raw markdown.
- Completed tasks keep their strikethrough while being edited.
- Task checkboxes and bullet glyphs scale with the editor text size setting.
- Clicking the glyph of a bullet or a scheduled or cancelled task places the cursor instead of doing nothing.
- Calendar day indicators sit closer under their date numbers and the rows are tighter.

### Fixed

- Pressing Enter on an empty line moves down a full line; blank lines no longer collapse when the cursor leaves them.
- On macOS the window close button now minimises Kairn to the Dock instead of leaving a stuck app that could only be force quit.
- Quick capture into a new day now follows the daily template rule, so days the template is set to skip (like weekends on the weekdays setting) stay plain.
- Typing into a day whose file already existed empty no longer wipes the note into a conflict banner.
- Editing an existing note no longer renames its file to match its first heading; only new untitled notes take their name from the typed title when saved or left.
- A new note whose title clashes with an existing note now shows a notice instead of silently keeping the Untitled name.
- The CLI's done command help no longer claims completed tasks get an @done timestamp.

## 0.2.1 (2026-08-10)

### Added

- The notes folder is chosen with the system folder picker instead of typing a path.
- A KAIRN_ROOT environment variable points the app at a different notes folder for a single run.
- Headings and bold text carry their own theme colours instead of rendering plain white.
- Three colour presets, Ocean, Rose, and Forest, join Dark and Light in the theme picker.
- Interface text size is adjustable in Settings, separately from the editor text size.
- The month and year above the calendar jump back to today and open today's note.

### Improved

- New notes open untitled with the cursor on the title line, and the file is renamed to match the title as it is written.
- The week strip shows the same tick and ring day indicators as the calendar.
- The calendar and week-strip arrows, folder triangles, and day numbers are larger and easier to see.
- The font pickers offer a curated set of installed families instead of every font on the system.
- The text selection highlight is clearer and no longer dims the selected text.
- Completing a task no longer writes an @done(date) stamp.
- The keybindings list in Settings shows larger, higher-contrast key chips.
- The titlebar has a Notes, Split, and Terminal layout switch, and no longer shows the Kairn name and mark.

### Fixed

- Dialog text fields take keyboard focus when the dialog opens.
- A squeezed dialog layout no longer scrolls a text field's content out of view.
- The daily template seeds upcoming days even when an empty daily note file already exists.
- Daily notes no longer carry a duplicate title line.

## 0.2.0 (2026-08-08)

The first tagged release of the Kairn v2 codebase: a ground-up rebuild in Rust and
GPUI, targeting macOS and Linux from one codebase. Source-only; packaged artifacts are
on the roadmap. Full notes in [release-notes/0.2.0.md](release-notes/0.2.0.md).

### Added

- Terminal mouse support for TUI apps: clicks, drags, and all three buttons are forwarded to applications that use the mouse (herdr, htop, lazygit).
- Paste into the terminal with the platform paste chord, with bracketed paste for apps that request it.
- Terminals report window focus changes to applications that ask for them.
- The Settings General tab shows the app version.

The previous macOS-only Swift app (Kairn v1, frozen) kept its own release notes in the
[kairn-v1](https://github.com/mikedclarke/kairn-v1) repository.
