# Contributing

Kairn is early and moving fast, so the process is deliberately light.

- **Bugs and ideas**: open a GitHub issue. For bugs, include your OS, how you launched
  Kairn, and what you expected to happen.
- **Small fixes**: pull requests welcome. Keep them focused on one change.
- **Bigger changes**: open an issue first so the approach can be agreed before you
  spend time on it.

## Development

Rust stable via [rustup](https://rustup.rs); edition 2024 needs a 2025+ toolchain.

```sh
cargo run                    # the app (binary kairn-app)
cargo run -p kairn-cli -- --help   # the kairn CLI
cargo test --workspace       # all tests
cargo fmt --all && cargo clippy --workspace   # before submitting
```

Platform setup (Linux package lists, fonts) is covered in the [README](README.md).

User-facing changes should add a bullet to `release-notes/UNRELEASED.md` following the
copy rules at the top of that file.
