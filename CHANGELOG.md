# Changelog

All notable user-facing changes to Kairn are recorded here, newest first. Changes land
in [release-notes/UNRELEASED.md](release-notes/UNRELEASED.md) as they are built and move
here when a version ships. The release process is described in
[docs/RELEASING.md](docs/RELEASING.md).

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
