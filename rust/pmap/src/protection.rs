// rust/pmap/src/protection.rs — Address-range and per-page protection changes.
use crate::types::*;
use crate::locking::PmapReadGuard;

pub fn pmap_protect(pmap: &mut Pmap, s: VmOffset, e: VmOffset, prot: VmProt) {
    if prot == VM_PROT_NONE { crate::mapping::pmap_remove(pmap, s, e); return; }
    if s >= e { return; }
    let mut guard = unsafe { PmapReadGuard::acquire(pmap) };
    let mut va = s;
    while va < e {
        let pte_ptr = crate::expand::pmap_pte(guard.pmap(), va);
        if !pte_ptr.is_null() {
            let pte = unsafe { *pte_ptr };
            if (pte & INTEL_PTE_VALID) != 0 {
                let new_bits = crate::mapping::intel_prot_to_pte_bits(prot);
                unsafe { *pte_ptr = (pte & !INTEL_PTE_WRITE) | (new_bits & INTEL_PTE_WRITE); }
            }
        }
        va += I386_PGBYTES; if va == 0 { break; }
    }
}

pub fn pmap_page_protect(phys: PhysAddr, prot: VmProt) {
    if !crate::pv_list::valid_page(phys) { return; }
    unsafe {
        let pai = crate::pv_list::pa_index(phys);
        let _pv_lock = crate::pv_list::lock_pvh(pai);
        let mut entry: *mut PvEntry = *pv_head_table.add(pai);
        while !entry.is_null() {
            let pmap_ptr = (*entry).pmap; let va = (*entry).va;
            if prot == VM_PROT_NONE {
                crate::mapping::pmap_remove(&mut *pmap_ptr, va, va + I386_PGBYTES);
            } else {
                let pte_ptr = crate::expand::pmap_pte(&*pmap_ptr, va);
                if !pte_ptr.is_null() && ((*pte_ptr & INTEL_PTE_VALID) != 0) {
                    *pte_ptr &= !INTEL_PTE_WRITE;
                }
            }
            entry = (*entry).next;
        }
    }
}

extern "C" { static mut pv_head_table: *mut *mut PvEntry; }
