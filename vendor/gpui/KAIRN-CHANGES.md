# Kairn changes to gpui

Vendored from crates.io `gpui 0.2.2` (the exact version the workspace already
resolved to; gpui-component and gpui-terminal must keep resolving to this same
gpui). Wired in via `[patch.crates-io]` in the root Cargo.toml.

Local changes, kept deliberately small:

- `src/text_system/line_wrapper.rs` and `src/text_system/line_layout.rs`
  (`compute_wrap_boundaries`, which duplicates the wrapper's logic): never
  record a wrap-boundary candidate directly before closing punctuation.
  Upstream treats any non-word character as a break opportunity, so a line
  ending in `word),` wrapped the `),` alone onto the next line. The new
  `LineWrapper::is_no_break_before` lists the closers (`) ] } > ! ? ; …` and
  their CJK forms); a word now travels together with the punctuation that
  trails it. `,`/`.`/`:` were already word characters upstream and never
  split from their word.

- `src/platform/linux/wayland/window.rs`, with the trait and core hooks in
  `src/platform.rs`, `src/window.rs` and `src/app.rs`: the Wayland frame loop
  parks when nothing needs drawing. Upstream requests a fresh
  `wl_surface.frame` callback and commits the surface on every callback, so an
  idle window woke the app and the compositor 60 times a second forever, with
  no buffer ever attached. Now `draw` records that a buffer was attached,
  `completed_frame` requests the next callback and commits only when one was,
  and a tick that draws nothing leaves the loop parked. `PlatformWindow` gained
  `request_redraw`, a no-op on macOS and X11, which un-parks the loop; core
  calls it wherever a window becomes dirty (`Window::refresh`,
  `Window::on_next_frame`, `App::apply_refresh_effect`, `App::notify`, and a
  sweep at the end of `App::flush_effects` for windows that were dirtied while
  taken out of `App::windows` for an update). The scheduled tick is spawned on
  the foreground executor rather than pushed straight onto the event loop's
  idle queue, because an idle inserted while the loop is already running its
  idles would not be picked up until the next wake-up. This is upstream Zed's
  `FrameLoop` design at the size our copy needs; upstream's presentation-retry
  and occlusion handling is not ported.

- `src/window.rs`: setting `GPUI_FRAME_STATS` to a non-empty value makes the
  frame-request handler print one line a second,
  `gpui frame stats: ticks=N draws=M draw_avg_ms=X.X draw_max_ms=Y.Y`, so idle
  cost can be measured on a machine without a profiler attached. Unset, it costs
  one `OnceLock` read per tick, and a parked window prints nothing at all,
  because the line is printed from a tick.

Packaging: `examples/` and its `[[example]]` targets, `Cargo.lock`,
`Cargo.toml.orig`, and the cargo registry marker files are dropped from the
vendored copy; nothing else is touched.
