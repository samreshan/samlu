//! The local hook-listener port. Fixed on purpose — see server/mod.rs's
//! module doc comment for why we don't silently fall back to a scanned port
//! on conflict.

pub const DEFAULT_PORT: u16 = 47823;
