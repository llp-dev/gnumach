// rust/pmap/src/page_attr.rs — Modified/reference bit tracking.
use crate::types::*;
use crate::locking::PmapWriteGuard;

const PHYS_MODIFIED:   PhysAddr = INTEL_PTE_MOD;
const PHYS_REFERENCED: PhysAddr = INTEL_PTE_REF;

extern "C" { static mut pmap_phys_attributes: *mut u8; static mut pv_head_table: *mut *mut PvEntry; }

fn phys_attribute_clear(phys: PhysAddr, bits: PhysAddr) {
    if !crate::pv_list::valid_page(phys) { return; }
    let _guard = unsafe { PmapWriteGuard::acquire() };
    let pai = crate::pv_list::pa_index(phys);
    unsafe { *pmap_phys_attributes.add(pai) &= !(bits as u8); }
    let _pv_lock = crate::pv_list::lock_pvh(pai);
    unsafe {
        let mut entry: *mut PvEntry = *pv_head_table.add(pai);
        while !entry.is_null() {
            let pte_ptr = crate::expand::pmap_pte(&*(*entry).pmap, (*entry).va);
            if !pte_ptr.is_null() { *pte_ptr &= !bits; }
            entry = (*entry).next;
        }
    }
}

fn phys_attribute_test(phys: PhysAddr, bits: PhysAddr) -> bool {
    if !crate::pv_list::valid_page(phys) { return false; }
    let pai = crate::pv_list::pa_index(phys);
    if (unsafe { *pmap_phys_attributes.add(pai) } & (bits as u8)) != 0 { return true; }
    let _pv_lock = crate::pv_list::lock_pvh(pai);
    unsafe {
        let mut entry: *mut PvEntry = *pv_head_table.add(pai);
        while !entry.is_null() {
            let pte_ptr = crate::expand::pmap_pte(&*(*entry).pmap, (*entry).va);
            if !pte_ptr.is_null() && ((*pte_ptr & bits) != 0) { return true; }
            entry = (*entry).next;
        }
    }
    false
}

pub fn pmap_clear_modify(phys: PhysAddr) { phys_attribute_clear(phys, PHYS_MODIFIED); }
pub fn pmap_is_modified(phys: PhysAddr) -> bool { phys_attribute_test(phys, PHYS_MODIFIED) }
pub fn pmap_clear_reference(phys: PhysAddr) { phys_attribute_clear(phys, PHYS_REFERENCED); }
pub fn pmap_is_referenced(phys: PhysAddr) -> bool { phys_attribute_test(phys, PHYS_REFERENCED) }
