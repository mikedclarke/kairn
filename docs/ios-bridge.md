# iOS bridge (KairnFFI)

The iPhone app runs on the same document and sync engine as the desktop, with no
reimplementation in Swift and no protocol drift. That guarantee is one
XCFramework, `KairnFFI`, built from this workspace via UniFFI.

## What it wraps

`crates/kairn-ffi` is a thin UniFFI facade over the two UI-free cores:

- **kairn-core** — the editable `NoteBuffer` (offset edits, undo/redo, the
  three-way merge/`reconcile` path), line/span parsing for styling, task
  toggling and rescheduling, and the vault's daily-note naming.
- **kairn-sync** — the sync value types the phone's background handlers consume
  (`SyncCycleReport`, `SyncEngineStatus`, `FfiSyncEvent`). These are designed in
  now so the live engine object slots into this same framework with the concrete
  transport, not a second bridge. kairn-sync is already compiled into
  the iOS slices today.

The wrappers hold no logic of their own beyond mapping types across the FFI, so
every invariant stays in the crate that owns it. The desktop app links
kairn-core and kairn-sync natively and never touches `kairn-ffi`.

### Offsets

Every offset crossing the boundary is a **UTF-8 byte offset**, matching
kairn-core exactly. TextKit works in UTF-16 code units, so the Swift editor
converts at the boundary with the bridge's `utf16ToByte` / `byteToUtf16`
(one tested pair, rather than every call site reinventing it).

## Building

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim   # once
scripts/build-ios-framework.sh                              # release, -> dist/ios
PROFILE=debug scripts/build-ios-framework.sh               # faster, unoptimised
OUT_DIR=/path scripts/build-ios-framework.sh               # place output elsewhere
```

The script cross-compiles both iOS slices, generates the Swift bindings in
library mode (from the in-crate `uniffi-bindgen`, so the generator never drifts
from the linked `uniffi` runtime), and assembles `KairnFFI.xcframework` plus
`kairn_ffi.swift`.

## Consuming it

The `kairn-mobile` repo carries a local Swift package, `KairnFFI/`, that wraps
the artifact (its `scripts/build-framework.sh` calls this script and drops the
output in place). The compiled `.xcframework` is a build artifact — gitignored
there, rebuilt when the Rust core changes — while the generated bindings are
committed so Swift editing needs no Rust toolchain. The generated module
compiles in Swift 5 language mode (UniFFI's output predates Swift 6 strict
concurrency); consumers are unaffected.

Smoke test (edit, undo, merge, parse, task toggle, offset conversion) runs in
the simulator:

```sh
cd KairnFFI && xcodebuild test -scheme KairnFFI \
    -destination 'platform=iOS Simulator,name=iPhone 17'
```
