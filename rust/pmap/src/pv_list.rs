// rust/pmap/src/pv_list.rs — Physical→virtual reverse mapping table maintenance.
use core::ffi::{c_int, c_void};
use crate::types::*;
use crate::locking::PvLockGuard;

extern "C" {
    fn kmem_cache_alloc(cache: *mut c_void) -> *mut c_void;
    fn kmem_cache_free(cache: *mut c_void, ptr: *mut c_void);
    fn simple_lock(l: *mut SimpleLock);
    fn simple_unlock(l: *mut SimpleLock);
    fn vm_page_table_index(pa: PhysAddr) -> usize;
    fn valid_page_c(pa: PhysAddr) -> c_int;

    static mut pv_list_cache: *mut c_void;
    static mut pv_free_list: *mut PvEntry;
    static mut pv_free_list_lock: SimpleLock;
}

pub fn pv_alloc() -> *mut PvEntry {
    unsafe {
        simple_lock(&mut pv_free_list_lock);
        let entry = pv_free_list;
        if !entry.is_null() { pv_free_list = (*entry).next; }
        simple_unlock(&mut pv_free_list_lock);
        if !entry.is_null() { return entry; }
        kmem_cache_alloc(pv_list_cache) as *mut PvEntry
    }
}

pub fn pv_free(entry: *mut PvEntry) {
    if entry.is_null() { return; }
    unsafe {
        simple_lock(&mut pv_free_list_lock);
        (*entry).next = pv_free_list;
        pv_free_list = entry;
        simple_unlock(&mut pv_free_list_lock);
    }
}

pub fn lock_pvh(index: usize) -> PvLockGuard {
    unsafe { PvLockGuard::acquire(index) }
}

#[inline]
pub fn pa_index(pa: PhysAddr) -> usize {
    unsafe { vm_page_table_index(pa) }
}

pub fn valid_page(pa: PhysAddr) -> bool {
    unsafe { valid_page_c(pa) != 0 }
}
