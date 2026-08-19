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

Packaging: `examples/` and its `[[example]]` targets, `Cargo.lock`,
`Cargo.toml.orig`, and the cargo registry marker files are dropped from the
vendored copy; nothing else is touched.
