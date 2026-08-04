# Kairn

A calm, native productivity app for macOS and Linux: daily notes, a knowledge base, quick
capture, a markdown editor, and real terminals (local shells + SSH), built on plain markdown
files on disk, with AI agents as first-class users of the same content.

A cairn is a stack of stones that marks a path. Kairn is where you leave markers (notes,
decisions, knowledge) for your future self and for your agents.

## Status

Ground-up v2 build, just started (2026-08-04). Nothing usable here yet.

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
