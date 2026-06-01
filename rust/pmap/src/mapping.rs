// rust/pmap/src/mapping.rs — Core virtual→physical mapping operations.
use core::ffi::c_int;
use crate::types::*;
use crate::locking::PmapReadGuard;
use crate::expand::{pmap_pte, pmap_expand, pmap_pde};

extern "C" {
    static mut kernel_virtual_start: VmOffset;
    static mut kernel_virtual_end: VmOffset;
    fn phystokv_fn(pa: PhysAddr) -> usize;
    fn phys_attribute_set_c(pa: PhysAddr, bit: PhysAddr);
}

// ─── PTE Helpers ─────────────────────────────────────────────────

#[inline] fn pte_is_valid(pte: PhysAddr) -> bool { (pte & INTEL_PTE_VALID) != 0 }
#[inline] fn pte_to_pa(pte: PhysAddr) -> PhysAddr { pte & INTEL_PTE_PFN }
#[inline] fn pa_to_pte(pa: PhysAddr) -> PhysAddr { pa & INTEL_PTE_PFN }

pub(crate) fn intel_prot_to_pte_bits(prot: VmProt) -> PhysAddr {
    let mut bits: PhysAddr = INTEL_PTE_USER;
    if (prot as u32 & VM_PROT_WRITE as u32) != 0 { bits |= INTEL_PTE_WRITE; }
    bits
}

#[cfg(not(feature = "xen"))]
fn write_pte(pte_ptr: *mut Pte, value: PhysAddr) { unsafe { *pte_ptr = value; } }
#[cfg(feature = "xen")]
fn write_pte(pte_ptr: *mut Pte, value: PhysAddr) {
    extern "C" { fn hypervisor_write_pte(ptr: *mut Pte, value: PhysAddr); }
    unsafe { hypervisor_write_pte(pte_ptr, value); }
}

// ─── pmap_extract() ──────────────────────────────────────────────

pub fn pmap_extract(pmap: &Pmap, va: VmOffset) -> Option<PhysAddr> {
    let pte_ptr = pmap_pte(pmap, va);
    if pte_ptr.is_null() { return None; }
    let pte = unsafe { *pte_ptr };
    if !pte_is_valid(pte) { return None; }
    Some(pte_to_pa(pte))
}

// ─── pmap_enter() ────────────────────────────────────────────────

pub fn pmap_enter(pmap: &mut Pmap, va: VmOffset, pa: PhysAddr, prot: VmProt, wired: bool) {
    let mut guard = unsafe { PmapReadGuard::acquire(pmap) };

    let pte_ptr = pmap_expand(guard.pmap_mut(), va);
    if pte_ptr.is_null() { panic!("pmap_enter: pmap_expand failed"); }

    let old_pte = unsafe { *pte_ptr };

    if pte_is_valid(old_pte) {
        let old_pa = pte_to_pa(old_pte);
        if old_pa == pa {
            let mut new_pte = (old_pte & INTEL_PTE_PFN) | INTEL_PTE_VALID | intel_prot_to_pte_bits(prot);
            if wired { new_pte |= INTEL_PTE_WIRED; }
            unsafe { *pte_ptr = new_pte; }
            return;
        }
        if crate::pv_list::valid_page(old_pa) {
            let pai = crate::pv_list::pa_index(old_pa);
            let _pv_lock = crate::pv_list::lock_pvh(pai);
            remove_pv_entry(old_pa, guard.pmap(), va);
        }
    } else {
        guard.pmap_mut().stats.resident_count += 1;
    }

    let mut new_pte = pa_to_pte(pa) | INTEL_PTE_VALID | intel_prot_to_pte_bits(prot);
    if wired { new_pte |= INTEL_PTE_WIRED; guard.pmap_mut().stats.wired_count += 1; }

    write_pte(pte_ptr, new_pte);

    if crate::pv_list::valid_page(pa) {
        let pai = crate::pv_list::pa_index(pa);
        let _pv_lock = crate::pv_list::lock_pvh(pai);
        add_pv_entry(pa, guard.pmap(), va);
    }
}

// ─── pmap_remove() ───────────────────────────────────────────────

pub fn pmap_remove(pmap: &mut Pmap, s: VmOffset, e: VmOffset) {
    if s >= e { return; }
    let mut guard = unsafe { PmapReadGuard::acquire(pmap) };
    let mut va = s;
    while va < e {
        let pde_ptr = pmap_pde(guard.pmap(), va);
        if pde_ptr.is_null() || !pte_is_valid(unsafe { *pde_ptr }) {
            va = ((va >> PDESHIFT) + 1) << PDESHIFT;
            if va == 0 { break; }
            continue;
        }
        let pt = unsafe { phystokv_fn(pte_to_pa(unsafe { *pde_ptr })) as *mut Pte };
        let start_idx = ptenum(va) as isize;
        let end_idx = core::cmp::min(ptenum(e.saturating_sub(1)) as isize + 1, (I386_PGBYTES / core::mem::size_of::<Pte>()) as isize);
        for i in start_idx..end_idx {
            let pte_ptr = unsafe { pt.offset(i) };
            let old_pte = unsafe { *pte_ptr };
            if !pte_is_valid(old_pte) { continue; }
            let pa = pte_to_pa(old_pte);
            if (old_pte & INTEL_PTE_MOD) != 0 { unsafe { phys_attribute_set_c(pa, INTEL_PTE_MOD); } }
            if (old_pte & INTEL_PTE_REF) != 0 { unsafe { phys_attribute_set_c(pa, INTEL_PTE_REF); } }
            if crate::pv_list::valid_page(pa) {
                let pai = crate::pv_list::pa_index(pa);
                let _pv_lock = crate::pv_list::lock_pvh(pai);
                remove_pv_entry(pa, guard.pmap(), va + (i as usize * I386_PGBYTES));
            }
            unsafe { *pte_ptr = 0; }
            guard.pmap_mut().stats.resident_count -= 1;
            if (old_pte & INTEL_PTE_WIRED) != 0 { guard.pmap_mut().stats.wired_count -= 1; }
        }
        va = ((va >> PDESHIFT) + 1) << PDESHIFT;
        if va == 0 { break; }
    }
}

pub fn pmap_change_wiring(pmap: &mut Pmap, v: VmOffset, wired: bool) {
    let pte_ptr = pmap_pte(pmap, v);
    if pte_ptr.is_null() { return; }
    unsafe {
        if wired {
            if (*pte_ptr & INTEL_PTE_WIRED) == 0 {
                *pte_ptr |= INTEL_PTE_WIRED;
                pmap.stats.wired_count += 1;
            }
        } else {
            if (*pte_ptr & INTEL_PTE_WIRED) != 0 {
                *pte_ptr &= !INTEL_PTE_WIRED;
                pmap.stats.wired_count -= 1;
            }
        }
    }
}

pub fn pmap_pageable(_pmap: &Pmap, _start: VmOffset, _end: VmOffset, _pageable: bool) { /* advisory, no-op */ }

pub fn pmap_map_bd(virt: VmOffset, start: PhysAddr, end: PhysAddr, prot: VmProt) -> VmOffset {
    let mut v = virt; let mut p = start;
    while p < end {
        unsafe { pmap_enter(&mut *kernel_pmap, v, p, prot, true); }
        v += I386_PGBYTES; p += I386_PGBYTES as PhysAddr;
    }
    v
}

// ─── pv_entry helpers ────────────────────────────────────────────

fn add_pv_entry(pa: PhysAddr, pmap: &Pmap, va: VmOffset) {
    let entry = crate::pv_list::pv_alloc();
    if entry.is_null() { return; }
    unsafe {
        let pai = crate::pv_list::pa_index(pa);
        (*entry).pmap = pmap as *const Pmap as *mut Pmap;
        (*entry).va = va;
        (*entry).next = *pv_head_table.add(pai);
        *pv_head_table.add(pai) = entry;
    }
}

fn remove_pv_entry(pa: PhysAddr, pmap: &Pmap, va: VmOffset) {
    unsafe {
        let pai = crate::pv_list::pa_index(pa);
        let head: *mut *mut PvEntry = pv_head_table.add(pai);
        let mut prev: *mut PvEntry = core::ptr::null_mut();
        let mut cur: *mut PvEntry = *head;
        while !cur.is_null() {
            if (*cur).pmap == pmap as *const Pmap as *mut Pmap && (*cur).va == va {
                if prev.is_null() { *head = (*cur).next; } else { (*prev).next = (*cur).next; }
                crate::pv_list::pv_free(cur);
                return;
            }
            prev = cur; cur = (*cur).next;
        }
    }
}

extern "C" { static mut kernel_pmap: *mut Pmap; static mut pv_head_table: *mut *mut PvEntry; }
use crate::types::ptenum;
