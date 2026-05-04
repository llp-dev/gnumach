//! gnumach ipc/ — Rust port (work in progress).
//!
//! Each module here mirrors a single ipc/<name>.c file.  Once a module is
//! complete, the corresponding C file is dropped from libkernel_a_SOURCES in
//! Makefrag.am (the C source itself is kept on disk for reference / rollback).

#![no_std]
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

/// Runtime assert — expands to a call to the kernel's `Assert(...)` (which
/// panics) when the condition is false.  Mirrors the C `assert(...)` macro
/// from `kern/assert.h` when `MACH_ASSERT` is enabled.
#[macro_export]
macro_rules! kassert {
    ($cond:expr, $msg:literal) => {
        if !($cond) {
            unsafe {
                $crate::extern_c::Assert(
                    concat!(file!(), "\0").as_ptr(),
                    line!() as core::ffi::c_int,
                    b"<rust>\0".as_ptr(),
                    concat!($msg, "\0").as_ptr(),
                );
            }
        }
    };
}

/// Unconditional kernel panic — mirrors the C `panic("...")` macro.
#[macro_export]
macro_rules! kpanic {
    ($msg:literal) => {
        unsafe {
            $crate::extern_c::Assert(
                concat!(file!(), "\0").as_ptr(),
                line!() as core::ffi::c_int,
                b"<rust>\0".as_ptr(),
                concat!($msg, "\0").as_ptr(),
            )
        }
    };
}

mod copy_user;
pub(crate) mod extern_c;
mod ipc_entry;
mod ipc_init;
mod ipc_kmsg;
mod ipc_marequest;
mod ipc_mqueue;
mod ipc_notify;
mod ipc_object;
mod ipc_port;
mod ipc_pset;
mod ipc_right;
mod ipc_space;
mod ipc_table;
mod ipc_target;
mod ipc_thread;
mod locks;
mod mach_debug;
mod mach_msg;
mod mach_port;
mod mach_types;

// The kernel's C `panic` is a macro that expands to `Panic(__FILE__, …)`,
// not a callable symbol.  Until we wire the panic_handler to call `Panic`
// with synthesized args, just halt.  The smoke test never panics, so the
// linker may still pull this stub in — it just has to not reference any
// external C symbol.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// `rust_eh_personality` is referenced (but never called) by some objects in
// the precompiled `core`/`compiler_builtins` rlibs.  With `panic=abort` it
// has no role; provide a stub so ld -r doesn't leave it undefined.
#[no_mangle]
pub unsafe extern "C" fn rust_eh_personality() {}

// LLVM lowers some short equality compares to `bcmp`, but the kernel C
// ships only `memcmp` (i386/i386/strings.c).  Provide a thin alias.
#[no_mangle]
pub unsafe extern "C" fn bcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    extern "C" {
        fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32;
    }
    unsafe { memcmp(s1, s2, n) }
}
