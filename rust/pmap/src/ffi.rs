// rust/pmap/src/ffi.rs — Complete `#[no_mangle] extern "C"` wrappers.
// Converts C types ↔ Rust types at the FFI boundary.

use core::ffi::{c_int, c_ulong, c_void};
use crate::types::*;

#[no_mangle] pub extern "C" fn pmap_virtual_space(startp: *mut VmOffset, endp: *mut VmOffset) { unsafe { *startp = kernel_virtual_start; *endp = kernel_virtual_end; } }
#[no_mangle] pub extern "C" fn pmap_create(size: VmSize) -> *mut Pmap { crate::lifecycle::pmap_create(size) }
#[no_mangle] pub extern "C" fn pmap_destroy(p: *mut Pmap) { crate::lifecycle::pmap_destroy(p); }
#[no_mangle] pub extern "C" fn pmap_reference(p: *mut Pmap) { crate::lifecycle::pmap_reference(p); }
#[no_mangle] pub extern "C" fn pmap_collect(p: *mut Pmap) { crate::lifecycle::pmap_collect(p); }
#[no_mangle] pub extern "C" fn pmap_kernel() -> *mut Pmap { unsafe { kernel_pmap } }
#[no_mangle] pub extern "C" fn pmap_resident_count(pmap: *mut Pmap) -> c_int { if pmap.is_null() { 0 } else { unsafe { (*pmap).stats.resident_count } } }

#[no_mangle] pub extern "C" fn pmap_enter(pmap: *mut Pmap, va: VmOffset, pa: PhysAddr, prot: VmProt, wired: c_int) { if !pmap.is_null() { crate::mapping::pmap_enter(unsafe { &mut *pmap }, va, pa, prot, wired != 0); } }
#[no_mangle] pub extern "C" fn pmap_remove(pmap: *mut Pmap, s: VmOffset, e: VmOffset) { if !pmap.is_null() { crate::mapping::pmap_remove(unsafe { &mut *pmap }, s, e); } }
#[no_mangle] pub extern "C" fn pmap_extract(pmap: *mut Pmap, va: VmOffset) -> PhysAddr { if pmap.is_null() { 0 } else { crate::mapping::pmap_extract(unsafe { &*pmap }, va).unwrap_or(0) } }
#[no_mangle] pub extern "C" fn pmap_protect(pmap: *mut Pmap, s: VmOffset, e: VmOffset, prot: VmProt) { if !pmap.is_null() { crate::protection::pmap_protect(unsafe { &mut *pmap }, s, e, prot); } }
#[no_mangle] pub extern "C" fn pmap_page_protect(phys: PhysAddr, prot: VmProt) { crate::protection::pmap_page_protect(phys, prot); }
#[no_mangle] pub extern "C" fn pmap_change_wiring(pmap: *mut Pmap, v: VmOffset, wired: c_int) { if !pmap.is_null() { crate::mapping::pmap_change_wiring(unsafe { &mut *pmap }, v, wired != 0); } }
#[no_mangle] pub extern "C" fn pmap_pageable(pmap: *mut Pmap, _s: VmOffset, _e: VmOffset, _p: c_int) { let _ = (pmap, _s, _e, _p); }
#[no_mangle] pub extern "C" fn pmap_map_bd(virt: VmOffset, start: PhysAddr, end: PhysAddr, prot: VmProt) -> VmOffset { crate::mapping::pmap_map_bd(virt, start, end, prot) }
#[no_mangle] pub extern "C" fn pmap_clear_modify(phys: PhysAddr) { crate::page_attr::pmap_clear_modify(phys); }
#[no_mangle] pub extern "C" fn pmap_is_modified(phys: PhysAddr) -> c_int { crate::page_attr::pmap_is_modified(phys) as c_int }
#[no_mangle] pub extern "C" fn pmap_clear_reference(phys: PhysAddr) { crate::page_attr::pmap_clear_reference(phys); }
#[no_mangle] pub extern "C" fn pmap_is_referenced(phys: PhysAddr) -> c_int { crate::page_attr::pmap_is_referenced(phys) as c_int }
#[no_mangle] pub extern "C" fn kvtophys(va: VmOffset) -> PhysAddr { crate::phys_ops::kvtophys(va) }
#[no_mangle] pub extern "C" fn copy_to_phys(src: VmOffset, dst: PhysAddr, count: c_int) { crate::phys_ops::copy_to_phys(src, dst, count); }
#[no_mangle] pub extern "C" fn copy_from_phys(src: PhysAddr, dst: VmOffset, count: c_int) { crate::phys_ops::copy_from_phys(src, dst, count); }
#[no_mangle] pub extern "C" fn pmap_get_mapwindow(entry: PhysAddr) -> *mut PmapMapwindow { crate::mapwindow::pmap_get_mapwindow(entry) }
#[no_mangle] pub extern "C" fn pmap_put_mapwindow(map: *mut PmapMapwindow) { crate::mapwindow::pmap_put_mapwindow(map); }
#[no_mangle] pub extern "C" fn pmap_activate(pmap: *mut Pmap, thread: Thread, cpu: c_int) { crate::activation::pmap_activate(pmap, thread, cpu); }
#[no_mangle] pub extern "C" fn pmap_deactivate(pmap: *mut Pmap, thread: Thread, cpu: c_int) { crate::activation::pmap_deactivate(pmap, thread, cpu); }
#[no_mangle] pub extern "C" fn pmap_pte(pmap: *mut Pmap, addr: VmOffset) -> *mut Pte { if pmap.is_null() { core::ptr::null_mut() } else { crate::expand::pmap_pte(unsafe { &*pmap }, addr) } }

#[cfg(feature = "kdb")]
#[no_mangle] pub extern "C" fn pmap_whatis(pmap: *mut Pmap, a: VmOffset) -> c_int { crate::whatis::pmap_whatis(pmap, a) }
#[cfg(feature = "smp")]
#[no_mangle] pub extern "C" fn signal_cpus(use_list: CpuSet, pmap: *mut Pmap, start: VmOffset, end: VmOffset) { crate::smp::signal_cpus(use_list, pmap, start, end); }
#[cfg(feature = "smp")]
#[no_mangle] pub extern "C" fn process_pmap_updates(my_pmap: *mut Pmap) { crate::smp::process_pmap_updates(my_pmap); }
#[cfg(feature = "smp")]
#[no_mangle] pub extern "C" fn pmap_update_interrupt() { crate::smp::pmap_update_interrupt(); }

/* pmap_map_mfn is defined in C (pmap_bootstrap.c) — Xen PV runtime function.
 * pmap_set_page_readwrite, pmap_set_page_readonly, pmap_set_page_readonly_init
 * are also defined in C. The Rust stubs are NOT exported to avoid duplicate symbols. */

