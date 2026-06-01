// rust/pmap/src/ffi.rs — C-compatible extern wrappers for all public PMAP symbols.
//
// Each function is a thin `#[no_mangle] pub extern "C"` trampoline that:
//  1. Converts C types to Rust types (raw ptr → &ref, c_int → bool, etc.)
//  2. Calls the idiomatic Rust implementation
//  3. Converts the return value back to C types

use core::ffi::c_int;

use crate::types::*;

/// Returns the kernel pmap pointer. Called by C code.
#[no_mangle]
pub extern "C" fn pmap_kernel() -> *mut Pmap {
    unsafe { kernel_pmap }
}

/// Returns the resident count for a pmap.
#[no_mangle]
pub extern "C" fn pmap_resident_count(pmap: *mut Pmap) -> c_int {
    if pmap.is_null() {
        return 0;
    }
    unsafe { (*pmap).stats.resident_count }
}
