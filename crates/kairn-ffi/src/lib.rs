//! The Rust-to-iOS bridge. A single UniFFI facade so the iPhone app drives the
//! exact same document core (and, once the transport lands, sync engine) as the
//! desktop, with no reimplementation on the Swift side and no protocol drift.
//!
//! The desktop app links `kairn-core` and `kairn-sync` directly and never sees
//! this crate; it exists only to cross the Swift boundary. Everything here is a
//! thin wrapper: the wrappers hold no logic of their own beyond mapping types
//! across the FFI, so the invariants stay in the crates that own them.
//!
//! Layout mirrors the pieces it bridges:
//! - [`buffer`]  the editable [`kairn_core::NoteBuffer`] as a UniFFI object
//! - [`parse`]   line/span classification for styling
//! - [`tasks`]   task toggling and rescheduling
//! - [`vault`]   the vault's daily-note naming convention
//! - [`text`]    UTF-16 <-> byte offset helpers for TextKit
//! - [`sync`]    the sync value types the phone's background handlers consume;
//!               the live engine object lands with the concrete transport
//!               (GDL-675), reusing this same framework.
//!
//! ## Offsets
//! Every offset crossing this boundary is a **UTF-8 byte offset**, matching
//! `kairn-core`'s native contract exactly (one engine, no drift). TextKit works
//! in UTF-16 code units, so the Swift editor converts at the boundary with
//! [`text::utf16_to_byte`] / [`text::byte_to_utf16`]. `u64` is used for offsets
//! across the FFI (native `usize` is 64-bit on every iOS device, so the cast is
//! lossless).

pub mod buffer;
pub mod merge;
pub mod parse;
pub mod sync;
pub mod tasks;
pub mod text;
pub mod vault;

uniffi::setup_scaffolding!();
