// SPDX-License-Identifier: GPL-2.0-or-later
//! Panic handler for the freestanding kernel staticlib.
//!
//! The kernel is built with `panic = "abort"`, so this handler never has to
//! unwind.  It forwards to the C kernel `panic()` so a Rust panic surfaces
//! exactly like any other fatal kernel error.

use core::panic::PanicInfo;

extern "C" {
    // C `panic(fmt, …)` is a macro for `Panic(file, line, func, fmt, …)`.
    fn Panic(file: *const u8, line: i32, func: *const u8, fmt: *const u8, ...) -> !;
}

#[panic_handler]
fn rust_panic(_info: &PanicInfo) -> ! {
    // Without an allocator we cannot easily format the message through C
    // varargs; a fixed string is enough to make the failure fatal and visible.
    unsafe {
        Panic(
            b"gnumach_rs\0".as_ptr(),
            0,
            b"rust_panic\0".as_ptr(),
            b"gnumach_rs: Rust code panicked\0".as_ptr(),
        )
    }
}

/// Language-item personality routine.  With `panic = "abort"` the unwinder is
/// never invoked, but the symbol can still be referenced by leftover landing-pad
/// metadata; provide a defined no-op so the kernel link sees no undefined
/// `rust_eh_personality` (which its undefined-symbol allowlist rejects).
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

extern "C" {
    fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32;
}

/// `bcmp` — byte compare, semantically `memcmp` for the equality test that
/// Rust's `core` slice/string code lowers to.  The kernel provides `memcmp`
/// but not `bcmp`, so define it here (returns 0 iff the regions are equal).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    unsafe { memcmp(s1, s2, n) }
}
