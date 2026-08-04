# Kairn

A calm, native productivity app for macOS and Linux: daily notes, a knowledge base, quick
capture, a markdown editor, and real terminals (local shells + SSH), built on plain markdown
files on disk, with AI agents as first-class users of the same content.

A cairn is a stack of stones that marks a path. Kairn is where you leave markers (notes,
decisions, knowledge) for your future self and for your agents.

## Status

Ground-up v2 build, just started (2026-08-04). Nothing usable here yet.

## Terminal spike

Minimal GPUI app: one window, one terminal running your login shell in a real PTY
(keyboard input, resize, 10k-line scrollback). This is the feel-test gate for the GPUI
stack decision.

Approach: [gpui-terminal](https://github.com/zortax/gpui-terminal) (alacritty_terminal
engine, portable-pty for the PTY), on the crates.io `gpui` release rather than a Zed git
checkout. Pinned versions:

- `gpui` 0.2.2 (crates.io)
- `gpui-terminal` git rev `51f0292938876c8da3de03f0139088591e3be518` (post-0.1.0 cell
  sizing and rendering fixes; still targets gpui 0.2.2)
- `alacritty_terminal` 0.25.1, `portable-pty` 0.9 (crates.io, resolved via Cargo.lock)

### Build and run

Both platforms need Rust (stable, via [rustup](https://rustup.rs)); edition 2024 requires
a 2025+ toolchain.

macOS: Xcode or the command-line tools, then

```sh
cargo run                # login shell ($SHELL)
cargo run -- ssh myhost  # any command in the PTY instead of the shell
```

Linux: GPUI renders via Vulkan and needs Wayland/X11 + fontconfig headers to build.
On Debian/Ubuntu:

```sh
sudo apt install build-essential cmake clang libfontconfig-dev libwayland-dev \
  libx11-xcb-dev libxkbcommon-x11-dev libvulkan1 mesa-vulkan-drivers libasound2-dev \
  libzstd-dev libssl-dev
cargo run
```

Fedora equivalents: `gcc clang cmake fontconfig-devel wayland-devel libxcb-devel
libxkbcommon-x11-devel vulkan-loader alsa-lib-devel libzstd-devel openssl-devel` plus
Mesa Vulkan drivers. The canonical dependency list is Zed's
[`script/linux`](https://github.com/zed-industries/zed/blob/main/script/linux).

Default font is Menlo on macOS, DejaVu Sans Mono on Linux; cmd/ctrl +/- adjusts font
size. The window closes when the shell exits.

The previous macOS-only Swift app lives at
[kairn-v1](https://github.com/mikedclarke/kairn-v1) (frozen; final build still served via
its appcast). This rebuild targets macOS and Linux from one codebase.

## Stack

- Rust + [GPUI](https://www.gpui.rs) (Zed's UI framework) + [gpui-component](https://github.com/longbridge/gpui-component)
- Terminal: alacritty_terminal engine, PTYs via portable-pty, SSH sessions
- Content: plain markdown files on disk, no database, no lock-in
- Agent API: localhost-only MCP + HTTP served from the app process

## Principles

1. Plain markdown files on disk; the app is a window over files the user owns.
2. Agents are users, not integrations.
3. Local-first, no servers: sync is Syncthing's job, remote machines are reached via SSH.
4. Calm UI: quiet, stable, no churning chrome.
5. One codebase, per-platform artifacts (.dmg for macOS, AppImage/deb for Linux).
