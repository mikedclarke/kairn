# Kairn changes to gpui-component

Vendored from crates.io `gpui-component 0.5.1` (the latest release at the time;
it must resolve to the same `gpui` as the rest of the workspace). Wired in via
`[patch.crates-io]` in the root Cargo.toml.

Local changes, kept deliberately small:

- `src/input/state.rs` `scroll_to`: guard against a degenerate zero-width
  `last_bounds`. When a squeezed layout hands the text element no width, the
  upstream math scrolls the entire text out of view on every click (the field
  renders blank). With the guard, a zero-width layout leaves the horizontal
  scroll offset alone.
