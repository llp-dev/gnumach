// rust/pmap/src/expand.rs — Page table walker and on-demand expansion.
//
// Central functions for navigating x86 page table hierarchies:
//   pmap_pte() — walk tables to find leaf PTE
//   pmap_pde() — walk to find page directory entry
//   pmap_expand() — allocate intermediate tables as needed
//
// Architecture variants handled via #[cfg]: 2-level (no PAE), 3-level (PAE 32-bit),
// 4-level (x86_64), with Xen PV differences.

use core::ffi::c_int;
use crate::types::*;

// ─── FFI to kernel helpers ───────────────────────────────────────

extern "C" {
    /// Convert physical address to kernel virtual address via direct map.
    fn phystokv(pa: PhysAddr) -> usize;

    /// Allocate a new physical page for a page-table page.
    fn pmap_page_table_page_alloc() -> *mut Pte;
}

/// Check if a PTE has the Valid (Present) bit set.
#[inline]
fn pte_is_valid(pte: PhysAddr) -> bool {
    (pte & INTEL_PTE_VALID) != 0
}

// ─── Page table base accessor ────────────────────────────────────

/// L4 base accessor (x86_64 only).
#[cfg(feature = "x86_64")]
#[inline]
unsafe fn pmap_l4base(pmap: &Pmap, _addr: VmOffset) -> *mut Pte {
    pmap.l4base
}

// ─── pmap_pte() — walk page table hierarchy to leaf PTE ──────────

/// Given a pmap and virtual address, walks the page table hierarchy
/// and returns a pointer to the leaf PTE, or null if the mapping is
/// incomplete (no intermediate page tables allocated).
pub fn pmap_pte(pmap: &Pmap, addr: VmOffset) -> *mut Pte {
    #[cfg(feature = "x86_64")]
    {
        // 4-level walk: L4 → PDP → PD → PT
        if pmap.l4base.is_null() {
            return core::ptr::null_mut();
        }

        // L4 lookup
        let l4_idx = lin2l4num(addr) as usize;
        let l4e = unsafe { *pmap.l4base.add(l4_idx) };
        if !pte_is_valid(l4e) {
            return core::ptr::null_mut();
        }

        // PDP lookup
        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdp_table = unsafe { phystokv(pte_to_pa(l4e)) as *mut Pte };
        let pdpe = unsafe { *pdp_table.add(pdp_idx) };
        if !pte_is_valid(pdpe) {
            return core::ptr::null_mut();
        }

        // PD lookup
        let pde_idx = lin2pdenum(addr) as usize;
        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        let pde = unsafe { *pd_table.add(pde_idx) };
        if !pte_is_valid(pde) {
            return core::ptr::null_mut();
        }

        // Large page check (PS bit)
        if (pde & INTEL_PTE_PS) != 0 {
            return unsafe { pd_table.add(pde_idx) };
        }

        // PT lookup — leaf PTE
        let pte_idx = ptenum(addr) as usize;
        let pt_table = unsafe { phystokv(pte_to_pa(pde)) as *mut Pte };
        return unsafe { pt_table.add(pte_idx) };
    }

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        // 3-level walk (32-bit PAE): PDP → PD → PT
        if pmap.pdpbase.is_null() {
            return core::ptr::null_mut();
        }

        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdpe = unsafe { *pmap.pdpbase.add(pdp_idx) };
        if !pte_is_valid(pdpe) {
            return core::ptr::null_mut();
        }

        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        let pde_idx = lin2pdenum_cont(addr) as usize;
        let pde = unsafe { *pd_table.add(pde_idx) };
        if !pte_is_valid(pde) {
            return core::ptr::null_mut();
        }

        if (pde & INTEL_PTE_PS) != 0 {
            return unsafe { pd_table.add(pde_idx) };
        }

        let pt_table = unsafe { phystokv(pte_to_pa(pde)) as *mut Pte };
        let pte_idx = ptenum(addr) as usize;
        return unsafe { pt_table.add(pte_idx) };
    }

    #[cfg(not(feature = "pae"))]
    {
        // 2-level walk: PD → PT
        if pmap.dirbase.is_null() {
            return core::ptr::null_mut();
        }

        let pde_idx = lin2pdenum(addr) as usize;
        let pde = unsafe { *pmap.dirbase.add(pde_idx) };
        if !pte_is_valid(pde) {
            return core::ptr::null_mut();
        }

        let pt_table = unsafe { phystokv(pte_to_pa(pde)) as *mut Pte };
        let pte_idx = ptenum(addr) as usize;
        return unsafe { pt_table.add(pte_idx) };
    }
}

// ─── pmap_pde() — walk to page directory entry ──────────────────

/// Get a pointer to the page directory entry for an address in a pmap.
/// Returns null if higher-level tables are missing.
pub fn pmap_pde(pmap: &Pmap, addr: VmOffset) -> *mut Pte {
    #[cfg(feature = "x86_64")]
    {
        if pmap.l4base.is_null() {
            return core::ptr::null_mut();
        }
        let l4_idx = lin2l4num(addr) as usize;
        let l4e = unsafe { *pmap.l4base.add(l4_idx) };
        if !pte_is_valid(l4e) {
            return core::ptr::null_mut();
        }

        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdp_table = unsafe { phystokv(pte_to_pa(l4e)) as *mut Pte };
        let pdpe = unsafe { *pdp_table.add(pdp_idx) };
        if !pte_is_valid(pdpe) {
            return core::ptr::null_mut();
        }

        let pde_idx = lin2pdenum(addr) as usize;
        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        unsafe { pd_table.add(pde_idx) }
    }

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        if pmap.pdpbase.is_null() {
            return core::ptr::null_mut();
        }
        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdpe = unsafe { *pmap.pdpbase.add(pdp_idx) };
        if !pte_is_valid(pdpe) {
            return core::ptr::null_mut();
        }

        let pde_idx = lin2pdenum_cont(addr) as usize;
        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        unsafe { pd_table.add(pde_idx) }
    }

    #[cfg(not(feature = "pae"))]
    {
        if pmap.dirbase.is_null() {
            return core::ptr::null_mut();
        }
        let pde_idx = lin2pdenum(addr) as usize;
        unsafe { pmap.dirbase.add(pde_idx) }
    }
}

// ─── pmap_expand() — on-demand page table allocation ────────────

/// Ensure a full page-table path exists for a virtual address.
/// Allocates intermediate page tables as needed.
/// Returns pointer to the leaf PTE.
///
/// # Panics
/// Panics if page-table page allocation fails (kernel OOM).
pub fn pmap_expand(pmap: &mut Pmap, v: VmOffset) -> *mut Pte {
    #[cfg(feature = "x86_64")]
    {
        // Expand L4 → PDP if needed
        let l4_idx = lin2l4num(v) as usize;
        if !pte_is_valid(unsafe { *pmap.l4base.add(l4_idx) }) {
            let new_pdp = unsafe { pmap_page_table_page_alloc() };
            if new_pdp.is_null() {
                panic!("pmap_expand: PDP alloc failed");
            }
            // Zero the page (done by pmap_page_table_page_alloc)
            // Create entry in L4: RW + US + Present, pointing to new PDP
            let pa = unsafe { kvtophys(new_pdp as VmOffset) };
            unsafe {
                *pmap.l4base.add(l4_idx) = pa_to_pte(pa)
                    | INTEL_PTE_VALID
                    | INTEL_PTE_WRITE
                    | INTEL_PTE_USER;
            }
        }

        // Expand PDP → PD if needed
        let pdp_idx = lin2pdpnum(v) as usize;
        let l4e = unsafe { *pmap.l4base.add(l4_idx) };
        let pdp_table = unsafe { phystokv(pte_to_pa(l4e)) as *mut Pte };
        if !pte_is_valid(unsafe { *pdp_table.add(pdp_idx) }) {
            let new_pd = unsafe { pmap_page_table_page_alloc() };
            if new_pd.is_null() {
                panic!("pmap_expand: PD alloc failed");
            }
            let pa = unsafe { kvtophys(new_pd as VmOffset) };
            unsafe {
                *pdp_table.add(pdp_idx) = pa_to_pte(pa)
                    | INTEL_PTE_VALID
                    | INTEL_PTE_WRITE
                    | INTEL_PTE_USER;
            }
        }

        // Expand PD → PT if needed
        let pde_idx = lin2pdenum(v) as usize;
        let pdpe = unsafe { *pdp_table.add(pdp_idx) };
        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        if !pte_is_valid(unsafe { *pd_table.add(pde_idx) }) {
            let new_pt = unsafe { pmap_page_table_page_alloc() };
            if new_pt.is_null() {
                panic!("pmap_expand: PT alloc failed");
            }
            let pa = unsafe { kvtophys(new_pt as VmOffset) };
            unsafe {
                *pd_table.add(pde_idx) = pa_to_pte(pa)
                    | INTEL_PTE_VALID
                    | INTEL_PTE_WRITE
                    | INTEL_PTE_USER;
            }
        }
    }

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        let pdp_idx = lin2pdpnum(v) as usize;
        if !pte_is_valid(unsafe { *pmap.pdpbase.add(pdp_idx) }) {
            let new_pd = unsafe { pmap_page_table_page_alloc() };
            if new_pd.is_null() {
                panic!("pmap_expand: PD alloc failed");
            }
            let pa = unsafe { kvtophys(new_pd as VmOffset) };
            unsafe {
                *pmap.pdpbase.add(pdp_idx) = pa_to_pte(pa)
                    | INTEL_PTE_VALID
                    | INTEL_PTE_WRITE
                    | INTEL_PTE_USER;
            }
        }

        let pd_table = unsafe {
            phystokv(pte_to_pa(unsafe { *pmap.pdpbase.add(pdp_idx) })) as *mut Pte
        };
        let pde_idx = lin2pdenum_cont(v) as usize;
        if !pte_is_valid(unsafe { *pd_table.add(pde_idx) }) {
            let new_pt = unsafe { pmap_page_table_page_alloc() };
            if new_pt.is_null() {
                panic!("pmap_expand: PT alloc failed");
            }
            let pa = unsafe { kvtophys(new_pt as VmOffset) };
            unsafe {
                *pd_table.add(pde_idx) = pa_to_pte(pa)
                    | INTEL_PTE_VALID
                    | INTEL_PTE_WRITE
                    | INTEL_PTE_USER;
            }
        }
    }

    #[cfg(not(feature = "pae"))]
    {
        let pde_idx = lin2pdenum(v) as usize;
        if !pte_is_valid(unsafe { *pmap.dirbase.add(pde_idx) }) {
            let new_pt = unsafe { pmap_page_table_page_alloc() };
            if new_pt.is_null() {
                panic!("pmap_expand: PT alloc failed");
            }
            let pa = unsafe { kvtophys(new_pt as VmOffset) };
            unsafe {
                *pmap.dirbase.add(pde_idx) = pa_to_pte(pa)
                    | INTEL_PTE_VALID
                    | INTEL_PTE_WRITE
                    | INTEL_PTE_USER;
            }
        }
    }

    // Now tables are allocated — return the leaf PTE
    pmap_pte(pmap, v)
}

// kvtophys helper
extern "C" {
    fn kvtophys(va: VmOffset) -> PhysAddr;
}
