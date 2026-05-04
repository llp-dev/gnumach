//! Port of `ipc/copy_user.c`.
//!
//! The C source has been empty since the USER32 removal — the user/kernel
//! address-size-mismatch helpers it once held are gone.  The Rust port is
//! consequently also empty.  Inline helpers (`copyin_address`, …) still live
//! in the C header `ipc/copy_user.h` and are inlined into C callers; no
//! Rust counterpart needed.
