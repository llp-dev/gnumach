// rust/pmap/src/lifecycle.rs — Pmap lifecycle: create, destroy, reference, collect.
use core::ffi::c_void;
use crate::types::*;

extern "C" {
    fn kmem_cache_alloc(cache: *mut c_void) -> *mut c_void;
    fn kmem_cache_free(cache: *mut c_void, ptr: *mut c_void);
    fn pmap_page_table_page_alloc_c() -> *mut Pte;
    fn phystokv(pa: PhysAddr) -> usize;

    static mut pmap_cache: *mut c_void;
    static mut kernel_pmap: *mut Pmap;
    static mut kernel_virtual_start: VmOffset;
    static mut kernel_virtual_end: VmOffset;
}

pub fn pmap_create(_size: VmSize) -> *mut Pmap {
    let pmap = unsafe { kmem_cache_alloc(pmap_cache) as *mut Pmap };
    if pmap.is_null() { return core::ptr::null_mut(); }
    unsafe {
        core::ptr::write_bytes(pmap, 0, 1);
        #[cfg(feature = "x86_64")]
        {
            (*pmap).l4base = pmap_page_table_page_alloc_c();
            if (*pmap).l4base.is_null() { kmem_cache_free(pmap_cache, pmap as *mut c_void); return core::ptr::null_mut(); }
            let kernel_l4 = (*kernel_pmap).l4base;
            let start = lin2l4num(kernel_virtual_start) as usize;
            let end = lin2l4num(kernel_virtual_end) as usize + 1;
            core::ptr::copy_nonoverlapping(kernel_l4.add(start), (*pmap).l4base.add(start), end.saturating_sub(start));
        }
        #[cfg(all(feature = "pae", not(feature = "x86_64")))]
        {
            (*pmap).pdpbase = pmap_page_table_page_alloc_c();
            if (*pmap).pdpbase.is_null() { kmem_cache_free(pmap_cache, pmap as *mut c_void); return core::ptr::null_mut(); }
        }
        #[cfg(not(feature = "pae"))]
        {
            (*pmap).dirbase = pmap_page_table_page_alloc_c();
            if (*pmap).dirbase.is_null() { kmem_cache_free(pmap_cache, pmap as *mut c_void); return core::ptr::null_mut(); }
        }
        (*pmap).ref_count = 1;
    }
    pmap
}

pub fn pmap_destroy(p: *mut Pmap) {
    if p.is_null() { return; }
    unsafe { (*p).ref_count -= 1; if (*p).ref_count > 0 { return; } }
    // Free page table pages would go here
    unsafe { kmem_cache_free(pmap_cache, p as *mut c_void); }
}

pub fn pmap_reference(p: *mut Pmap) {
    if !p.is_null() { unsafe { (*p).ref_count += 1; } }
}

pub fn pmap_collect(_p: *mut Pmap) { /* stub */ }
use crate::types::lin2l4num;
