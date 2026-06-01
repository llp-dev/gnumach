// rust/pmap/src/types.rs — Core type definitions for the PMAP module.
//
// All structs crossing the C-Rust boundary use #[repr(C)].
// PTE bit constants match i386/intel/pmap.h definitions.

use core::ffi::{c_int, c_uint, c_ulong, c_void};

// ─── Type aliases matching C kernel types ────────────────────────

pub type VmOffset = usize;      // vm_offset_t → uintptr_t
pub type VmSize = usize;        // vm_size_t → uintptr_t
pub type Integer = c_int;       // integer_t
pub type Natural = c_uint;      // natural_t

#[cfg(not(feature = "pae"))]
pub type PhysAddr = u32;        // unsigned long (non-PAE 32-bit)
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
pub type PhysAddr = u64;        // unsigned long long (PAE 32-bit)
#[cfg(feature = "x86_64")]
pub type PhysAddr = u64;        // unsigned long long (x86_64)

pub type Pte = PhysAddr;        // pt_entry_t ≡ phys_addr_t
pub type VmProt = c_int;        // vm_prot_t
pub type CpuSet = c_ulong;      // volatile long bitmap
pub type KernReturn = c_int;    // kern_return_t

// Opaque C pointers — typed via bindgen in real build, void* for now
pub type Thread = *mut c_void;
pub type VmMap = *mut c_void;
pub type VmObject = *mut c_void;
pub type VmPage = *mut c_void;
pub type KmCache = *mut c_void;

// ─── PTE bit definitions (from i386/intel/pmap.h) ────────────────

pub const INTEL_PTE_VALID:  PhysAddr = 0x0000_0001;
pub const INTEL_PTE_WRITE:  PhysAddr = 0x0000_0002;
pub const INTEL_PTE_USER:   PhysAddr = 0x0000_0004;
pub const INTEL_PTE_WTHRU:  PhysAddr = 0x0000_0008;
pub const INTEL_PTE_NCACHE: PhysAddr = 0x0000_0010;
pub const INTEL_PTE_REF:    PhysAddr = 0x0000_0020;
pub const INTEL_PTE_MOD:    PhysAddr = 0x0000_0040;
pub const INTEL_PTE_PS:     PhysAddr = 0x0000_0080;

#[cfg(not(feature = "xen"))]
pub const INTEL_PTE_GLOBAL: PhysAddr = 0x0000_0100;
#[cfg(feature = "xen")]
pub const INTEL_PTE_GLOBAL: PhysAddr = 0x0000_0000; // not supported under Xen PV

pub const INTEL_PTE_WIRED:  PhysAddr = 0x0000_0200;
pub const INTEL_PTE_EXECUTE: PhysAddr = 0; // NX handled via separate mechanism on x86_64

#[cfg(not(feature = "pae"))]
pub const INTEL_PTE_PFN:    PhysAddr = 0x00_FFFF_F000;
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
pub const INTEL_PTE_PFN:    PhysAddr = 0x00_7FFF_FFFF_FFFF_F000;
#[cfg(feature = "x86_64")]
pub const INTEL_PTE_PFN:    PhysAddr = 0x000F_FFFF_FFFF_F000;

pub const INTEL_OFFMASK:    PhysAddr = 0xFFF;

// ─── Page size constants ─────────────────────────────────────────

pub const I386_PGSHIFT: u32 = 12;
pub const I386_PGBYTES: usize = 1 << I386_PGSHIFT;

// ─── Page table level shift/mask constants ───────────────────────

#[cfg(not(feature = "pae"))]
pub const PDESHIFT: u32 = 22;
#[cfg(not(feature = "pae"))]
pub const PDEMASK: u32 = 0x3FF;
#[cfg(not(feature = "pae"))]
pub const PTEMASK: u32 = 0x3FF;

#[cfg(feature = "pae")]
pub const PDPSHIFT: u32 = 30;
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
pub const PDPMASK: u32 = 3;
#[cfg(all(feature = "pae", feature = "x86_64"))]
pub const PDPMASK: u32 = 0x1FF;
#[cfg(feature = "pae")]
pub const PDESHIFT: u32 = 21;
#[cfg(feature = "pae")]
pub const PDEMASK: u32 = 0x1FF;
#[cfg(feature = "pae")]
pub const PTEMASK: u32 = 0x1FF;

pub const PTESHIFT: u32 = 12;

#[cfg(feature = "x86_64")]
pub const L4SHIFT: u32 = 39;
#[cfg(feature = "x86_64")]
pub const L4MASK: u32 = 0x1FF;

// ─── Simple lock type (repr(C), matches decl_simple_lock_data) ───

#[repr(C)]
#[derive(Default)]
pub struct SimpleLock {
    pub lock_data: c_uint,
}

// ─── Pmap statistics (from mach/vm_statistics.h) ─────────────────

#[repr(C)]
#[derive(Default)]
pub struct PmapStatistics {
    pub resident_count: Integer,
    pub wired_count: Integer,
}

// ─── Main Pmap struct (repr(C), matches struct pmap in pmap.h) ───

#[repr(C)]
pub struct Pmap {
    #[cfg(not(feature = "pae"))]
    pub dirbase: *mut Pte,

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    pub pdpbase: *mut Pte,

    #[cfg(feature = "x86_64")]
    pub l4base: *mut Pte,
    #[cfg(all(feature = "xen_hyp", feature = "x86_64"))]
    pub user_l4base: *mut Pte,
    #[cfg(all(feature = "xen_hyp", feature = "x86_64"))]
    pub user_pdpbase: *mut Pte,

    pub ref_count: c_int,
    pub lock: SimpleLock,
    pub stats: PmapStatistics,
    pub cpus_using: CpuSet,
}

// ─── PV entry for reverse mapping table ──────────────────────────

#[repr(C)]
pub struct PvEntry {
    pub next: *mut PvEntry,
    pub pmap: *mut Pmap,
    pub va: VmOffset,
}

// ─── Map window structure ────────────────────────────────────────

#[repr(C)]
pub struct PmapMapwindow {
    pub entry: *mut Pte,
    pub vaddr: VmOffset,
}

// ─── VM protection constants ─────────────────────────────────────

pub const VM_PROT_NONE:    VmProt = 0x00;
pub const VM_PROT_READ:    VmProt = 0x01;
pub const VM_PROT_WRITE:   VmProt = 0x02;
pub const VM_PROT_EXECUTE: VmProt = 0x04;
pub const VM_PROT_ALL:     VmProt = VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE;

// ─── Kernel return codes ─────────────────────────────────────────

pub const KERN_SUCCESS:            KernReturn = 0;
pub const KERN_INVALID_ADDRESS:    KernReturn = 1;
pub const KERN_PROTECTION_FAILURE: KernReturn = 2;
pub const KERN_RESOURCE_SHORTAGE:  KernReturn = 6;

// ─── PTE math utility functions ──────────────────────────────────

/// Convert physical address to PTE value (mask to PFN).
#[inline]
pub fn pa_to_pte(pa: PhysAddr) -> PhysAddr {
    pa & INTEL_PTE_PFN
}

/// Extract physical address from PTE value.
#[inline]
pub fn pte_to_pa(pte: PhysAddr) -> PhysAddr {
    pte & INTEL_PTE_PFN
}

/// Increment PTE by one page.
#[inline]
pub fn pte_increment_pa(pte: &mut PhysAddr) {
    *pte += INTEL_OFFMASK + 1;
}

/// Check if PTE is valid (present).
#[inline]
pub fn pte_is_valid(pte: PhysAddr) -> bool {
    (pte & INTEL_PTE_VALID) != 0
}

// ─── Page-table index math ───────────────────────────────────────

/// Convert linear address to L4 table index (x86_64 only).
#[cfg(feature = "x86_64")]
#[inline]
pub fn lin2l4num(addr: VmOffset) -> u32 {
    ((addr >> L4SHIFT) & L4MASK as usize) as u32
}

/// Convert linear address to page directory pointer index (PAE only).
#[cfg(feature = "pae")]
#[inline]
pub fn lin2pdpnum(addr: VmOffset) -> u32 {
    ((addr >> PDPSHIFT) & PDPMASK as usize) as u32
}

/// Convert linear address to page directory entry index.
#[inline]
pub fn lin2pdenum(addr: VmOffset) -> u32 {
    ((addr >> PDESHIFT) & PDEMASK as usize) as u32
}

/// Convert linear address to page table entry index.
#[inline]
pub fn ptenum(addr: VmOffset) -> u32 {
    ((addr >> PTESHIFT) & PTEMASK as usize) as u32
}

/// Convert page descriptor entry index to linear address offset.
#[inline]
pub fn pdenum2lin(pde_idx: u32) -> VmOffset {
    (pde_idx as VmOffset) << PDESHIFT
}

/// PAE-specific: contiguous PDE index (includes PDP offset).
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
#[inline]
pub fn lin2pdenum_cont(addr: VmOffset) -> u32 {
    ((addr >> PDESHIFT) & 0x7FF) as u32
}

/// Reconstruct page-aligned linear address from table indices.
pub fn pagenum2lin(_l4: u32, _pdp: u32, pde: u32, pte: u32) -> VmOffset {
    #[cfg(feature = "x86_64")]
    {
        ((_l4 as VmOffset) << L4SHIFT)
        + ((_pdp as VmOffset) << PDPSHIFT)
        + ((pde as VmOffset) << PDESHIFT)
        + ((pte as VmOffset) << PTESHIFT)
    }
    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        ((_pdp as VmOffset) << PDPSHIFT)
        + ((pde as VmOffset) << PDESHIFT)
        + ((pte as VmOffset) << PTESHIFT)
    }
    #[cfg(not(feature = "pae"))]
    {
        ((pde as VmOffset) << PDESHIFT)
        + ((pte as VmOffset) << PTESHIFT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pte_to_pa_roundtrip() {
        let test_pas = &[0u64, 0x1000, 0x12345000, 0xFFFFF000];
        for &pa in test_pas {
            let pa = pa as PhysAddr;
            let pte = pa_to_pte(pa);
            let back = pte_to_pa(pte);
            assert_eq!(back, pa, "round-trip failed for pa {:#x}", pa);
        }
    }

    #[test]
    fn test_pte_bit_constants_no_overlap() {
        let flags = INTEL_PTE_VALID
                  | INTEL_PTE_WRITE
                  | INTEL_PTE_USER
                  | INTEL_PTE_WTHRU
                  | INTEL_PTE_NCACHE
                  | INTEL_PTE_REF
                  | INTEL_PTE_MOD
                  | INTEL_PTE_PS
                  | INTEL_PTE_GLOBAL
                  | INTEL_PTE_WIRED;

        assert_eq!(flags & 0xFFF, flags);
        assert_eq!(flags & INTEL_PTE_PFN, 0);
    }

    #[test]
    fn test_page_size_constants() {
        assert_eq!(I386_PGBYTES, 4096);
        assert_eq!(1 << I386_PGSHIFT, I386_PGBYTES);
    }

    #[test]
    fn test_pte_increment() {
        let mut pte: PhysAddr = 0x1000;
        let orig = pte;
        pte_increment_pa(&mut pte);
        assert_eq!(pte, orig + 0x1000);
        assert_eq!(pte_to_pa(pte), 0x2000);
    }

    #[test]
    fn test_pte_valid() {
        assert!(pte_is_valid(INTEL_PTE_VALID));
        assert!(pte_is_valid(0x1001));
        assert!(!pte_is_valid(0x1000));
        assert!(!pte_is_valid(0));
    }

    #[test]
    fn test_pte_to_pa_masks_correctly() {
        let pte: PhysAddr = 0x12345000 | INTEL_PTE_VALID | INTEL_PTE_WRITE | INTEL_PTE_MOD;
        assert_eq!(pte_to_pa(pte), 0x12345000);
    }

    #[test]
    fn test_pa_to_pte_masks_correctly() {
        let pa: PhysAddr = 0x12345FFF;
        let pte = pa_to_pte(pa);
        assert_eq!(pte, 0x12345000);
        assert_eq!(pte & 0xFFF, 0);
    }

    #[test]
    fn test_ptenum() {
        assert_eq!(ptenum(0x0000_0000), 0);
        assert_eq!(ptenum(0x0000_1000), 1);
        assert_eq!(ptenum(0x0000_2000), 2);
        assert_eq!(ptenum(0x003F_F000), 0x3FF);
    }

    #[test]
    fn test_lin2pdenum_default() {
        let addr: VmOffset = 0x0040_0000;
        assert_eq!(lin2pdenum(addr), 1);
        assert_eq!(lin2pdenum(0), 0);
    }

    #[test]
    fn test_pdenum2lin() {
        assert_eq!(pdenum2lin(1), 1_usize << PDESHIFT);
        assert_eq!(pdenum2lin(0), 0);
    }

    #[test]
    fn test_offset_mask() {
        assert_eq!(INTEL_OFFMASK, 0xFFF);
        assert_eq!(INTEL_OFFMASK + 1, I386_PGBYTES as PhysAddr);
    }
}
