// SPDX-License-Identifier: GPL-2.0-or-later
//! GNU Mach kernel components implemented in Rust.
//!
//! This crate is compiled to a `staticlib` (`libkernel-rust.a`) and linked
//! into the `gnumach` kernel.  It currently provides the i386 `pmap`
//! (physical map) module, a drop-in replacement for `i386/intel/pmap.c`.
//!
//! The crate is `#![no_std]`: it runs in the freestanding kernel environment
//! and calls back into the C kernel for memory allocation, page management,
//! TLB shootdown helpers, panic/printf, etc.  Every public entry point is
//! `#[unsafe(no_mangle)] pub extern "C"` so it transparently replaces the
//! original C symbol at link time.
#![no_std]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

mod panic;

#[path = "i386/pmap.rs"]
pub mod pmap;
