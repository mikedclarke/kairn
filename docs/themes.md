# Theme files

Kairn ships two built-in themes, Dark and Light. Custom themes are JSON
files in `.kairn/themes/` inside the notes folder, so they sync with the
vault. Pick them in Settings → Theme; the file stem is the theme's id.

A theme names a `mode` ("dark" or "light") and overrides any subset of the
palette, fonts, and terminal colors. Everything left out falls back to the
built-in theme for that mode, so a minimal file is a valid theme. Colors
are `#rrggbb` or `#rrggbbaa` (the alpha form is useful for `sel` and
`highlight`, which draw over text).

```json
{
  "name": "Gruvbox",
  "mode": "dark",
  "colors": {
    "bg": "#282828",
    "panel": "#3c3836",
    "panel2": "#32302f",
    "hover": "#3c3836",
    "border": "#504945",
    "text": "#ebdbb2",
    "dim": "#bdae93",
    "faint": "#928374",
    "accent": "#b8bb26",
    "amber": "#fabd2f",
    "on_amber": "#282828",
    "red": "#fb4934",
    "term_bg": "#1d2021",
    "sel": "#b8bb2626",
    "highlight": "#fabd2f48",
    "heading": "#fe8019"
  },
  "fonts": {
    "ui": "Avenir Next",
    "editor": "iA Writer Quattro S",
    "mono": "JetBrains Mono",
    "editor_size": 14
  },
  "terminal": {
    "background": "#1d2021",
    "foreground": "#ebdbb2",
    "cursor": "#b8bb26",
    "red": "#fb4934",
    "green": "#b8bb26",
    "yellow": "#fabd2f",
    "blue": "#83a598",
    "magenta": "#d3869b",
    "cyan": "#8ec07c"
  }
}
```

Field notes:

- `colors.highlight` is the `==highlight==` background, alpha respected;
  when unset it derives from `amber` at 28% over whatever the file set.
- `colors.heading` colors markdown headings and the note masthead;
  unset follows `text`.
- `fonts` mirror the Settings → Theme pickers; a font set directly in
  settings wins over the theme file's choice. `editor_size` is the body
  size in px (clamped 9–32); headings scale with it.
- `terminal` covers `background`, `foreground`, `cursor`, the eight ANSI
  colors, and their `bright_*` variants. Unset fields keep the built-in
  sage ramp, with the background following `colors.term_bg`.

Malformed files are skipped (listed on stderr) and a theme that fails to
load falls back to Dark, so a bad edit can't lock the app into an
unreadable state.
