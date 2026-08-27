//! Host hooks for filament's transport layer.
//!
//! # What this is, and what it is not yet
//!
//! The transport ladder itself (`net.rs`, `direct.rs`: the `Transport` trait,
//! direct QUIC, WebRTC, relay failover) still lives in the CLI. This crate holds
//! the thing that was BLOCKING it from leaving: the handful of places where
//! connection code reached sideways into terminal output, user settings, config
//! paths and interface enumeration.
//!
//! Those files now call `hooks::` instead of `crate::ui` / `crate::doctor` /
//! `crate::settings` / `crate::platform` / `crate::interact`. With no CLI
//! references left in them, moving them here becomes a file move rather than a
//! refactor, which is deliberate: the transport is what the whole reliability
//! story rests on, and a half-verified transport is worse than a coupled one.
//!
//! # Why function pointers rather than a trait object
//!
//! These are process-global facts, not per-connection policy. Threading a
//! `&dyn Host` through 4,000 lines of connection code would be a far larger and
//! riskier change than the coupling it removes. Every hook has a safe default,
//! so a consumer that installs nothing still gets a working transport with the
//! diagnostics simply discarded.

pub mod hooks;
