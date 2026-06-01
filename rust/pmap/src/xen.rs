// rust/pmap/src/xen.rs — Xen PV page table operations (stub, delegates to C).
use core::ffi::{c_ulong, c_void};

pub fn pmap_set_page_readwrite(addr: *mut c_void) {
    extern "C" { fn pmap_set_page_readwrite_c(addr: *mut c_void); }
    unsafe { pmap_set_page_readwrite_c(addr); }
}

pub fn pmap_set_page_readonly(addr: *mut c_void) {
    extern "C" { fn pmap_set_page_readonly_c(addr: *mut c_void); }
    unsafe { pmap_set_page_readonly_c(addr); }
}

pub fn pmap_set_page_readonly_init(addr: *mut c_void) {
    extern "C" { fn pmap_set_page_readonly_init_c(addr: *mut c_void); }
    unsafe { pmap_set_page_readonly_init_c(addr); }
}

pub fn pmap_map_mfn(addr: *mut c_void, mfn: c_ulong) {
    extern "C" { fn pmap_map_mfn_c(addr: *mut c_void, mfn: c_ulong); }
    unsafe { pmap_map_mfn_c(addr, mfn); }
}
