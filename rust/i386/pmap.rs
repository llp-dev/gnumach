// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 1985-1991 Carnegie Mellon University
//
//! i386 `pmap` — physical map management, migrated from `i386/intel/pmap.c`.
//!
//! This is an idiomatic-Rust reimplementation that replaces the C `pmap.c`
//! symbol-for-symbol.  Every public function is
//! `#[unsafe(no_mangle)] pub extern "C"` and keeps the exact C ABI, so the
//! rest of the (C) kernel links against it unchanged.
//!
//! ## Build configuration assumed
//!
//! This file targets the kernel's **default i386 configuration**:
//! `PAE` **undefined**, `NCPUS == 1`, `MACH_HYP`/`MACH_PV_PAGETABLES`
//! **undefined**, `MACH_SLOCKS == 0`, `LINUX_DEV` **defined**.  Consequences,
//! all mirrored here:
//!
//! * `pt_entry_t` is a 32-bit word; paging is 2-level (`PDESHIFT == 22`,
//!   `PDEMASK == PTEMASK == 0x3ff`, `INTEL_PTE_PFN == 0xfffff000`,
//!   `PDPNUM == 1`).
//! * `ptes_per_vm_page == 1`, so the C `do { … } while (--i > 0)` PTE loops
//!   collapse to a single store.
//! * On a uniprocessor the `simple_lock`s, `SPLVM`/`SPLX` and `LOCK_PVH`
//!   are no-ops; `struct pmap`'s embedded lock is zero-sized.  TLB updates
//!   only ever touch the local CPU.
//!
//! Line references in the doc comments point at the original `pmap.c` /
//! `pmap.h` they were ported from.

use core::ffi::{c_int, c_void};

// ───────────────────────── C type aliases ──────────────────────────

/// `pmap_t` = `struct pmap *` (i386/intel/pmap.h:228).
type pmap_t = *mut Pmap;
/// `vm_offset_t` = `uintptr_t` (mach/i386/vm_types.h).
type vm_offset_t = usize;
/// `vm_size_t` = `uintptr_t`.
type vm_size_t = usize;
/// `phys_addr_t` = `unsigned long` on the non-PAE i386 build (vm_types.h:91).
type phys_addr_t = u32;
/// `pt_entry_t` = `phys_addr_t` (i386/intel/pmap.h:67) — 32-bit here.
type pt_entry_t = u32;
/// `boolean_t` = `int` (mach/i386/boolean.h).
type boolean_t = c_int;
/// `vm_prot_t` = `int` (mach/vm_prot.h).
type vm_prot_t = c_int;
/// `cpu_set` = `volatile long` (i386/intel/pmap.h:204) — 32-bit on i386.
type cpu_set = i32;
/// `integer_t` = `int`.
type integer_t = c_int;
/// `kern_return_t` = `int`.
type kern_return_t = c_int;

// ───────────────────────── Layout-critical structs ─────────────────
//
// Verified against the configured build with `__builtin_offsetof`:
//   sizeof(struct pmap)        == 20
//   offsetof(ref_count)        ==  4
//   offsetof(stats)            ==  8
//   offsetof(cpus_using)       == 16
//   sizeof(embedded lock)      ==  0   (MACH_SLOCKS == 0)
//   sizeof(pmap_mapwindow_t)   ==  8
//   sizeof(struct pv_entry)    == 12

/// `struct pmap_statistics` (mach/vm_statistics.h:69).
#[repr(C)]
#[derive(Clone, Copy)]
struct PmapStatistics {
    resident_count: integer_t,
    wired_count: integer_t,
}

/// `struct pmap` (i386/intel/pmap.h:207), non-PAE layout.
///
/// The embedded `decl_simple_lock_data(,lock)` expands to a zero-sized
/// `struct simple_lock_data_empty` when `MACH_SLOCKS == 0`; modelled with
/// `[u8; 0]` so `stats`/`cpus_using` keep their measured offsets.
#[repr(C)]
struct Pmap {
    dirbase: *mut pt_entry_t,
    ref_count: c_int,
    _lock: [u8; 0],
    stats: PmapStatistics,
    cpus_using: cpu_set,
}

/// `struct pv_entry` / `pv_entry_t` (pmap.c:113).
#[repr(C)]
struct PvEntry {
    next: *mut PvEntry,
    pmap: pmap_t,
    va: vm_offset_t,
}

/// `pmap_mapwindow_t` (i386/intel/pmap.h:261).
#[repr(C)]
pub struct PmapMapwindow {
    entry: *mut pt_entry_t,
    vaddr: vm_offset_t,
}

// ───────────────────────── Constants (config.h / pmap.h) ───────────

/// `NCPUS` (config.h) — uniprocessor build.
const NCPUS: usize = 1;
/// `PAGE_SIZE` / `INTEL_PGBYTES` on i386 (mach/i386/vm_param.h).
const PAGE_SIZE: usize = 4096;
const INTEL_PGBYTES: usize = 4096;
const PAGE_MASK: usize = PAGE_SIZE - 1;
/// `BYTE_SIZE` (mach/i386/vm_param.h).
const BYTE_SIZE: usize = 8;

/// `PMAP_NMAPWINDOWS` (i386/intel/pmap.h:269).
const PMAP_NMAPWINDOWS: usize = 2;
/// `MAPWINDOW_SIZE` (pmap.c:447).
const MAPWINDOW_SIZE: vm_offset_t = PMAP_NMAPWINDOWS * NCPUS * PAGE_SIZE;

/// `ptes_per_vm_page` (pmap.c:431) — fixed at 1 on i386.
const PTES_PER_VM_PAGE: usize = 1;

// Paging geometry — non-PAE branch (i386/intel/pmap.h:88-94, 186).
const PDESHIFT: u32 = 22;
const PDEMASK: usize = 0x3ff;
const PTESHIFT: u32 = 12;
const PTEMASK: usize = 0x3ff;
/// `NPTES` = `INTEL_PGBYTES / sizeof(pt_entry_t)` (pmap.h:157).
const NPTES: usize = INTEL_PGBYTES / core::mem::size_of::<pt_entry_t>();
/// `PDPNUM` (pmap.h:89) — single page directory, non-PAE.
const PDPNUM: usize = 1;

// PTE bits (i386/intel/pmap.h:165-188).
const INTEL_PTE_VALID: pt_entry_t = 0x00000001;
const INTEL_PTE_WRITE: pt_entry_t = 0x00000002;
const INTEL_PTE_USER: pt_entry_t = 0x00000004;
const INTEL_PTE_WTHRU: pt_entry_t = 0x00000008;
const INTEL_PTE_NCACHE: pt_entry_t = 0x00000010;
const INTEL_PTE_REF: pt_entry_t = 0x00000020;
const INTEL_PTE_MOD: pt_entry_t = 0x00000040;
const INTEL_PTE_GLOBAL: pt_entry_t = 0x00000100;
const INTEL_PTE_WIRED: pt_entry_t = 0x00000200;
/// `INTEL_PTE_PFN` (pmap.h:187), non-PAE.
const INTEL_PTE_PFN: pt_entry_t = 0xfffff000;
/// `INTEL_OFFMASK` (pmap.h:72).
const INTEL_OFFMASK: vm_offset_t = 0xfff;

/// `PHYS_MODIFIED` / `PHYS_REFERENCED` (pmap.c:189-190).
const PHYS_MODIFIED: pt_entry_t = INTEL_PTE_MOD;
const PHYS_REFERENCED: pt_entry_t = INTEL_PTE_REF;

/// `PDE_MAPPED_SIZE` = `pdenum2lin(1)` = `1 << PDESHIFT` (pmap.c:196).
const PDE_MAPPED_SIZE: vm_offset_t = 1 << PDESHIFT;

// vm_prot_t (mach/vm_prot.h).
const VM_PROT_READ: vm_prot_t = 0x1;
const VM_PROT_WRITE: vm_prot_t = 0x2;
const VM_PROT_EXECUTE: vm_prot_t = 0x4;
const VM_PROT_ALL: vm_prot_t = VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE;

/// `KERN_SUCCESS` (mach/kern_return.h).
const KERN_SUCCESS: kern_return_t = 0;
/// `KMEM_CACHE_PHYSMEM` (kern/slab.h).
const KMEM_CACHE_PHYSMEM: c_int = 0x2;
/// `CPU_TYPE_I486` (mach/machine.h).
const CPU_TYPE_I486: integer_t = 17;

// CPU feature indices (i386/i386/cpu_number.h / proc_reg.h via CPU_HAS_FEATURE).
const CPU_FEATURE_PGE: u32 = 13;

// Address-space layout (i386/i386/vm_param.h) for the default i386-at build:
//   VM_MIN_KERNEL_ADDRESS == LINEAR_MIN_KERNEL_ADDRESS == 0xC0000000
//   INIT_VM_MIN_KERNEL_ADDRESS == 0   (non-Xen)
//   VM_MAX_KERNEL_ADDRESS == 0xFFFFFFFF
//   VM_KERNEL_MAP_SIZE == 170 MiB
const VM_MIN_KERNEL_ADDRESS: vm_offset_t = 0xC000_0000;
const LINEAR_MIN_KERNEL_ADDRESS: vm_offset_t = VM_MIN_KERNEL_ADDRESS;
const INIT_VM_MIN_KERNEL_ADDRESS: vm_offset_t = 0x0000_0000;
const VM_MAX_KERNEL_ADDRESS: vm_offset_t = 0xFFFF_FFFF;
const VM_MAX_USER_ADDRESS: vm_offset_t = 0xC000_0000;
const VM_KERNEL_MAP_SIZE: vm_offset_t = 170 * 1024 * 1024;

// ───────────────────────── C kernel imports ────────────────────────

extern "C" {
    // Debug / output.  The C `panic(fmt, …)` is a macro expanding to
    // `Panic(__FILE__, __LINE__, __FUNCTION__, fmt, …)`, so we call `Panic`.
    fn Panic(file: *const u8, line: c_int, func: *const u8, fmt: *const u8, ...) -> !;
    fn printf(fmt: *const u8, ...) -> c_int;

    // Slab caches (kern/slab.h).
    fn kmem_cache_init(
        cache: *mut c_void,
        name: *const u8,
        obj_size: vm_size_t,
        align: vm_size_t,
        ctor: *const c_void,
        flags: c_int,
    );
    fn kmem_cache_alloc(cache: *mut c_void) -> vm_offset_t;
    fn kmem_cache_free(cache: *mut c_void, obj: vm_offset_t);

    // Wired kernel allocation (vm/vm_kern.h).
    fn kmem_alloc_wired(map: *mut c_void, addr: *mut vm_offset_t, size: vm_size_t)
        -> kern_return_t;

    // VM page bookkeeping (vm/vm_page.h).
    fn vm_page_lookup_pa(pa: phys_addr_t) -> *mut c_void;
    fn vm_page_table_size() -> vm_size_t;
    fn vm_page_table_index(pa: phys_addr_t) -> vm_size_t;
    fn vm_page_ready() -> c_int;

    // Boot-time helpers (i386/i386at/biosmem.h, vm/pmap.h, model_dep.h).
    fn biosmem_directmap_end() -> phys_addr_t;
    fn pmap_grab_page() -> vm_offset_t;

    // libc-ish (kern/strings or compiler builtins).
    fn memset(s: *mut c_void, c: c_int, n: vm_size_t) -> *mut c_void;
    fn memcpy(d: *mut c_void, s: *const c_void, n: vm_size_t) -> *mut c_void;

    // CPU feature bitmap + machine slots.
    static cpu_features: [u32; 0];
    static machine_slot: [MachineSlot; NCPUS];

    // Page-fault continuation sentinel used by VM_PAGE_WAIT.
    fn vm_page_wait(continuation: *const c_void);

    // External kernel map handle.
    static kernel_map: *mut c_void;
}

/// Minimal view of `struct machine_slot` (mach/machine.h): we only read
/// `cpu_type`, which is the first `integer_t` field.
#[repr(C)]
struct MachineSlot {
    // `boolean_t is_cpu` then `cpu_type_t cpu_type` … but the C code only
    // dereferences `.cpu_type`.  The real struct begins with two
    // `unsigned int` bitfields packed before cpu_type; to read cpu_type
    // safely we replicate the leading layout precisely below.
    //
    // struct machine_slot {
    //     boolean_t  is_cpu;       /* int      */
    //     cpu_type_t cpu_type;     /* int      */
    //     ...
    // };
    is_cpu: integer_t,
    cpu_type: integer_t,
}

// ───────────────────────── Address / PTE helpers ───────────────────

/// `CPU_HAS_FEATURE` (i386/i386/proc_reg.h): test bit in `cpu_features`.
#[inline]
unsafe fn cpu_has_feature(feature: u32) -> bool {
    let words = core::ptr::addr_of!(cpu_features) as *const u32;
    unsafe { (*words.add((feature / 32) as usize) & (1u32 << (feature % 32))) != 0 }
}

/// `phystokv(a)` (i386/i386/vm_param.h:131).
#[inline]
fn phystokv(a: phys_addr_t) -> vm_offset_t {
    a as vm_offset_t + VM_MIN_KERNEL_ADDRESS
}

/// `_kvtophys(a)` (vm_param.h:135) — only valid for the 1-1 direct map.
#[inline]
fn _kvtophys(a: vm_offset_t) -> phys_addr_t {
    (a - VM_MIN_KERNEL_ADDRESS) as phys_addr_t
}

/// `kvtolin(a)` (vm_param.h:140) — identity here (the two bases are equal).
#[inline]
fn kvtolin(a: vm_offset_t) -> vm_offset_t {
    a - VM_MIN_KERNEL_ADDRESS + LINEAR_MIN_KERNEL_ADDRESS
}

/// `pa_to_pte(a)` (pmap.h:190).
#[inline]
fn pa_to_pte(a: phys_addr_t) -> pt_entry_t {
    a & INTEL_PTE_PFN
}

/// `pte_to_pa(p)` (pmap.h:194).
#[inline]
fn pte_to_pa(p: pt_entry_t) -> phys_addr_t {
    p & INTEL_PTE_PFN
}

/// `ptetokv(a)` (pmap.h:201).
#[inline]
fn ptetokv(a: pt_entry_t) -> vm_offset_t {
    phystokv(pte_to_pa(a))
}

/// `lin2pdenum(a)` (pmap.h:106).
#[inline]
fn lin2pdenum(a: vm_offset_t) -> usize {
    (a >> PDESHIFT) & PDEMASK
}

/// `lin2pdenum_cont(a)` (pmap.h:117) — identical to `lin2pdenum` non-PAE.
#[inline]
fn lin2pdenum_cont(a: vm_offset_t) -> usize {
    lin2pdenum(a)
}

/// `ptenum(a)` (pmap.h:155).
#[inline]
fn ptenum(a: vm_offset_t) -> usize {
    (a >> PTESHIFT) & PTEMASK
}

/// `pdenum2lin(a)` (pmap.h:130).
#[inline]
fn pdenum2lin(a: usize) -> vm_offset_t {
    (a as vm_offset_t) << PDESHIFT
}

/// `intel_btop(x)` (mach/i386/vm_param.h).
#[inline]
fn intel_btop(x: vm_offset_t) -> vm_offset_t {
    x >> PTESHIFT
}

/// `pv_lock_table_size(n)` (pmap.c:157).
#[inline]
fn pv_lock_table_size(n: vm_size_t) -> vm_size_t {
    (n + BYTE_SIZE - 1) / BYTE_SIZE
}

/// `round_page(x)` (mach/vm_param.h).
#[inline]
fn round_page(x: vm_size_t) -> vm_size_t {
    (x + PAGE_MASK) & !PAGE_MASK
}

/// `pa_index(pa)` (pmap.c:174).
#[inline]
fn pa_index(pa: phys_addr_t) -> usize {
    unsafe { vm_page_table_index(pa) }
}

/// `pai_to_pvh(pai)` (pmap.c:176).
#[inline]
fn pai_to_pvh(pai: usize) -> *mut PvEntry {
    unsafe { pv_head_table.add(pai) }
}

// ───────────────────────── %cr3 / TLB primitives ───────────────────

/// `get_cr3()` (i386/i386/proc_reg.h).
#[inline]
unsafe fn get_cr3() -> u32 {
    let v: u32;
    unsafe { core::arch::asm!("mov %cr3, {0}", out(reg) v, options(att_syntax, nostack, preserves_flags)) };
    v
}

/// `set_cr3(v)` (proc_reg.h).
#[inline]
unsafe fn set_cr3(v: u32) {
    unsafe { core::arch::asm!("mov {0}, %cr3", in(reg) v, options(att_syntax, nostack, preserves_flags)) };
}

/// `flush_tlb()` (proc_reg.h) — reload `%cr3`.
#[inline]
unsafe fn flush_tlb() {
    unsafe { set_cr3(get_cr3()) };
}

/// `invlpg_linear(start)` (proc_reg.h): invalidate one page through the
/// kernel's linear-mapping segment (`%es` = `LINEAR_DS`).
#[inline]
unsafe fn invlpg_linear(start: vm_offset_t) {
    const LINEAR_DS: u32 = 0x38;
    const KERNEL_DS: u32 = 0x10;
    unsafe {
        core::arch::asm!(
            "mov {lin}, %es",
            "invlpg %es:({addr})",
            "mov {kd}, %es",
            lin = in(reg) LINEAR_DS,
            addr = in(reg) start,
            kd = in(reg) KERNEL_DS,
            options(att_syntax, nostack, preserves_flags),
        );
    }
}

/// `INVALIDATE_TLB(pmap, s, e)` (pmap.c:360), uniprocessor / non-PV.
///
/// A single-page range is invalidated with `invlpg`; anything wider falls
/// back to a full TLB flush.
#[inline]
unsafe fn invalidate_tlb(pmap: pmap_t, s: vm_offset_t, e: vm_offset_t) {
    if e - s == PAGE_SIZE {
        let lin = if pmap == unsafe { kernel_pmap } { kvtolin(s) } else { s };
        unsafe { invlpg_linear(lin) };
    } else {
        unsafe { flush_tlb() };
    }
}

/// `PMAP_UPDATE_TLBS(pmap, s, e)` (pmap.c:338), NCPUS == 1.
#[inline]
unsafe fn pmap_update_tlbs(pmap: pmap_t, s: vm_offset_t, e: vm_offset_t) {
    if unsafe { (*pmap).cpus_using } != 0 {
        unsafe { invalidate_tlb(pmap, s, e) };
    }
}

// ───────────────────────── pv free-list helpers ────────────────────

/// `PV_ALLOC(pv_e)` (pmap.c:131) — pop the free list (lock is a UP no-op).
#[inline]
unsafe fn pv_alloc() -> *mut PvEntry {
    unsafe {
        let e = pv_free_list;
        if !e.is_null() {
            pv_free_list = (*e).next;
        }
        e
    }
}

/// `PV_FREE(pv_e)` (pmap.c:140) — push onto the free list.
#[inline]
unsafe fn pv_free(pv_e: *mut PvEntry) {
    unsafe {
        (*pv_e).next = pv_free_list;
        pv_free_list = pv_e;
    }
}

/// Small helper to abort like the C `panic()` with a fixed message.
/// `msg` must be NUL-terminated by the caller.
#[inline]
fn pmap_panic(msg: &[u8]) -> ! {
    unsafe {
        Panic(
            b"rust/i386/pmap.rs\0".as_ptr(),
            0,
            b"pmap\0".as_ptr(),
            msg.as_ptr(),
        )
    }
}

// ───────────────────────── Globals (defined in pmap.c) ─────────────
//
// These were file-scope globals in pmap.c.  They keep their C symbol names
// (`#[no_mangle]`) so the rest of the kernel — which `extern`s several of
// them (kernel_pmap, kernel_page_dir, kernel_virtual_start/end) — links
// against these definitions.

/// Backing storage for slab-cache handles (`struct kmem_cache`, 128 B / 64-aligned).
#[repr(C, align(64))]
struct KmemCacheStorage([u8; 128]);

/// `struct pmap kernel_pmap_store` (pmap.c:412).
#[unsafe(no_mangle)]
pub static mut kernel_pmap_store: Pmap = Pmap {
    dirbase: core::ptr::null_mut(),
    ref_count: 0,
    _lock: [],
    stats: PmapStatistics { resident_count: 0, wired_count: 0 },
    cpus_using: 0,
};

/// `pmap_t kernel_pmap` (pmap.c:413).
#[unsafe(no_mangle)]
pub static mut kernel_pmap: pmap_t = core::ptr::null_mut();

/// `pt_entry_t *kernel_page_dir` (pmap.c:440).
#[unsafe(no_mangle)]
pub static mut kernel_page_dir: *mut pt_entry_t = core::ptr::null_mut();

/// `vm_offset_t kernel_virtual_start` (pmap.c:167).
#[unsafe(no_mangle)]
pub static mut kernel_virtual_start: vm_offset_t = 0;
/// `vm_offset_t kernel_virtual_end` (pmap.c:168).
#[unsafe(no_mangle)]
pub static mut kernel_virtual_end: vm_offset_t = 0;

/// `boolean_t pmap_initialized` (pmap.c:160).
#[unsafe(no_mangle)]
pub static mut pmap_initialized: boolean_t = 0;

/// `boolean_t pmap_debug` (pmap.c:425).
#[unsafe(no_mangle)]
pub static mut pmap_debug: boolean_t = 0;

/// `unsigned int inuse_ptepages_count` (pmap.c:434).
#[unsafe(no_mangle)]
pub static mut inuse_ptepages_count: core::ffi::c_uint = 0;

/// `pv_entry_t pv_head_table` (pmap.c:121).
#[unsafe(no_mangle)]
pub static mut pv_head_table: *mut PvEntry = core::ptr::null_mut();
/// `pv_entry_t pv_free_list` (pmap.c:128).
#[unsafe(no_mangle)]
pub static mut pv_free_list: *mut PvEntry = core::ptr::null_mut();
/// `char *pv_lock_table` (pmap.c:156).
#[unsafe(no_mangle)]
pub static mut pv_lock_table: *mut u8 = core::ptr::null_mut();
/// `char *pmap_phys_attributes` (pmap.c:184).
#[unsafe(no_mangle)]
pub static mut pmap_phys_attributes: *mut u8 = core::ptr::null_mut();

/// `vm_object_t pmap_object` (pmap.c:202) — unused on the non-Xen build.
#[unsafe(no_mangle)]
pub static mut pmap_object: *mut c_void = core::ptr::null_mut();

/// `static pmap_mapwindow_t mapwindows[PMAP_NMAPWINDOWS * NCPUS]` (pmap.c:446).
static mut MAPWINDOWS: [PmapMapwindow; PMAP_NMAPWINDOWS * NCPUS] = [
    PmapMapwindow { entry: core::ptr::null_mut(), vaddr: 0 },
    PmapMapwindow { entry: core::ptr::null_mut(), vaddr: 0 },
];

/// `struct kmem_cache pmap_cache` (pmap.c:415).
#[unsafe(no_mangle)]
pub static mut pmap_cache: KmemCacheStorage = KmemCacheStorage([0; 128]);
/// `struct kmem_cache pt_cache` (pmap.c:416).
#[unsafe(no_mangle)]
pub static mut pt_cache: KmemCacheStorage = KmemCacheStorage([0; 128]);
/// `struct kmem_cache pd_cache` (pmap.c:417).
#[unsafe(no_mangle)]
pub static mut pd_cache: KmemCacheStorage = KmemCacheStorage([0; 128]);
/// `struct kmem_cache pv_list_cache` (pmap.c:148).
#[unsafe(no_mangle)]
pub static mut pv_list_cache: KmemCacheStorage = KmemCacheStorage([0; 128]);

// `_start[]` / `etext[]` linker symbols delimiting the kernel text.
extern "C" {
    #[link_name = "_start"]
    static KERNEL_START: u8;
    #[link_name = "etext"]
    static KERNEL_ETEXT: u8;
}

// ───────────────────────── Page-table walk helpers ─────────────────

/// `pmap_pde(pmap, addr)` (pmap.c:478), non-PAE: index the single page dir.
#[inline]
fn pmap_pde(pmap: pmap_t, addr: vm_offset_t) -> *mut pt_entry_t {
    let addr = if pmap == unsafe { kernel_pmap } { kvtolin(addr) } else { addr };
    let page_dir = unsafe { (*pmap).dirbase };
    if page_dir.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { page_dir.add(lin2pdenum(addr)) }
}

/// `pmap_pte(pmap, addr)` (pmap.c:506): resolve a VA to its PTE, or NULL.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_pte(pmap: pmap_t, addr: vm_offset_t) -> *mut pt_entry_t {
    unsafe {
        if (*pmap).dirbase.is_null() {
            return core::ptr::null_mut();
        }
    }
    let ptp = pmap_pde(pmap, addr);
    if ptp.is_null() {
        return core::ptr::null_mut();
    }
    let pte = unsafe { *ptp };
    if (pte & INTEL_PTE_VALID) == 0 {
        return core::ptr::null_mut();
    }
    let table = ptetokv(pte) as *mut pt_entry_t;
    unsafe { table.add(ptenum(addr)) }
}

// ───────────────────────── valid_page ──────────────────────────────

/// `valid_page(addr)` (pmap.c:1175): is `addr` within managed physical memory?
#[inline]
fn valid_page(addr: phys_addr_t) -> bool {
    if unsafe { pmap_initialized } == 0 {
        return false;
    }
    !unsafe { vm_page_lookup_pa(addr) }.is_null()
}

// ───────────────────────── Bootstrap ───────────────────────────────

/// `pmap_bootstrap()` (pmap.c:736): build the kernel page directory and the
/// 1-1 direct map of physical memory, with paging still off.  Non-PAE.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_bootstrap() {
    unsafe {
        // The kernel pmap is statically allocated.
        kernel_pmap = core::ptr::addr_of_mut!(kernel_pmap_store);
        // simple_lock_init is a no-op on UP.
        (*kernel_pmap).ref_count = 1;

        kernel_virtual_start = phystokv(biosmem_directmap_end());
        kernel_virtual_end = kernel_virtual_start.wrapping_add(VM_KERNEL_MAP_SIZE);
        if kernel_virtual_end < kernel_virtual_start
            || kernel_virtual_end > VM_MAX_KERNEL_ADDRESS - PAGE_SIZE
        {
            kernel_virtual_end = VM_MAX_KERNEL_ADDRESS - PAGE_SIZE;
        }

        printf(
            b"kernel virtual area: %lx-%lx\n\0".as_ptr(),
            kernel_virtual_start as core::ffi::c_ulong,
            kernel_virtual_end as core::ffi::c_ulong,
        );

        // Allocate and clear the kernel page directory (non-PAE: one page).
        let kpd = phystokv(pmap_grab_page() as phys_addr_t) as *mut pt_entry_t;
        (*kernel_pmap).dirbase = kpd;
        kernel_page_dir = kpd;
        for i in 0..(PDPNUM * NPTES) {
            *kpd.add(i) = 0;
        }

        let global: pt_entry_t = if cpu_has_feature(CPU_FEATURE_PGE) {
            INTEL_PTE_GLOBAL
        } else {
            0
        };

        let start_text = core::ptr::addr_of!(KERNEL_START) as vm_offset_t;
        let etext = core::ptr::addr_of!(KERNEL_ETEXT) as vm_offset_t;
        let directmap_end = phystokv(biosmem_directmap_end());

        // Map all directly-mappable physical memory 1-1; allocate spare
        // all-null page tables for later kernel VM allocation.
        let base = phystokv(0);
        let mut va: vm_offset_t = base;
        while va >= base && va < kernel_virtual_end {
            let pde = kpd.add(lin2pdenum_cont(kvtolin(va)));
            let ptable = phystokv(pmap_grab_page() as phys_addr_t) as *mut pt_entry_t;

            // Page-directory entry pointing at the new page table.
            *pde = pa_to_pte(_kvtophys(ptable as vm_offset_t))
                | INTEL_PTE_VALID
                | INTEL_PTE_WRITE;

            // Fill the page table for the direct-mapped region.
            let ptable_end = ptable.add(NPTES);
            let mut pte = ptable;
            while va < directmap_end && pte < ptable_end {
                if (pte as vm_offset_t - ptable as vm_offset_t) / core::mem::size_of::<pt_entry_t>()
                    < ptenum(va)
                {
                    *pte = 0;
                } else {
                    // Kernel text is mapped read-only; the rest read-write.
                    let entry = if va >= start_text && va + INTEL_PGBYTES <= etext {
                        pa_to_pte(_kvtophys(va)) | INTEL_PTE_VALID | global
                    } else {
                        pa_to_pte(_kvtophys(va)) | INTEL_PTE_VALID | INTEL_PTE_WRITE | global
                    };
                    *pte = entry;
                    va += INTEL_PGBYTES;
                }
                pte = pte.add(1);
            }
            // Remaining PTEs of this table: null, and record the map windows.
            while pte < ptable_end {
                if va >= kernel_virtual_end - MAPWINDOW_SIZE && va < kernel_virtual_end {
                    let idx = (va - (kernel_virtual_end - MAPWINDOW_SIZE)) >> PTESHIFT;
                    let win = &raw mut MAPWINDOWS[idx];
                    (*win).entry = pte;
                    (*win).vaddr = va;
                }
                *pte = 0;
                va += INTEL_PGBYTES;
                pte = pte.add(1);
            }
        }
        // Architecture-specific code turns on paging shortly after we return.
    }
}

// ───────────────────────── Map windows ─────────────────────────────

/// `pmap_get_mapwindow(entry)` (pmap.c:1047): install a temporary kernel PTE.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_get_mapwindow(entry: pt_entry_t) -> *mut PmapMapwindow {
    // cpu_number() == 0 on UP.
    unsafe {
        let base = 0; // cpu * PMAP_NMAPWINDOWS
        let mut i = base;
        while i < base + PMAP_NMAPWINDOWS {
            let win = &raw mut MAPWINDOWS[i];
            if *(*win).entry == 0 {
                *(*win).entry = entry;
                invalidate_tlb(kernel_pmap, (*win).vaddr, (*win).vaddr + PAGE_SIZE);
                return win;
            }
            i += 1;
        }
        pmap_panic(b"pmap_get_mapwindow: no free window\0");
    }
}

/// `pmap_put_mapwindow(map)` (pmap.c:1073): tear down a temporary mapping.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_put_mapwindow(map: *mut PmapMapwindow) {
    unsafe {
        *(*map).entry = 0;
        invalidate_tlb(kernel_pmap, (*map).vaddr, (*map).vaddr + PAGE_SIZE);
    }
}

/// `pmap_virtual_space(startp, endp)` (pmap.c:1084).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_virtual_space(startp: *mut vm_offset_t, endp: *mut vm_offset_t) {
    unsafe {
        *startp = kernel_virtual_start;
        *endp = kernel_virtual_end - MAPWINDOW_SIZE;
    }
}

// ───────────────────────── pmap_init ───────────────────────────────

/// `pmap_init()` (pmap.c:1097): allocate the pv table, attribute array and
/// the slab caches; mark the module initialized.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_init() {
    unsafe {
        let npages = vm_page_table_size();
        let mut s = core::mem::size_of::<PvEntry>() * npages
            + pv_lock_table_size(npages)
            + npages;
        s = round_page(s);

        let mut addr: vm_offset_t = 0;
        if kmem_alloc_wired(kernel_map, &mut addr, s) != KERN_SUCCESS {
            pmap_panic(b"pmap_init\0");
        }
        memset(addr as *mut c_void, 0, s);

        pv_head_table = addr as *mut PvEntry;
        addr = pv_head_table.add(npages) as vm_offset_t;

        pv_lock_table = addr as *mut u8;
        addr += pv_lock_table_size(npages);

        pmap_phys_attributes = addr as *mut u8;

        kmem_cache_init(
            core::ptr::addr_of_mut!(pmap_cache) as *mut c_void,
            b"pmap\0".as_ptr(),
            core::mem::size_of::<Pmap>(),
            0,
            core::ptr::null(),
            0,
        );
        kmem_cache_init(
            core::ptr::addr_of_mut!(pt_cache) as *mut c_void,
            b"pmap_L1\0".as_ptr(),
            INTEL_PGBYTES,
            INTEL_PGBYTES,
            core::ptr::null(),
            KMEM_CACHE_PHYSMEM,
        );
        kmem_cache_init(
            core::ptr::addr_of_mut!(pd_cache) as *mut c_void,
            b"pmap_L2\0".as_ptr(),
            INTEL_PGBYTES,
            INTEL_PGBYTES,
            core::ptr::null(),
            KMEM_CACHE_PHYSMEM,
        );
        kmem_cache_init(
            core::ptr::addr_of_mut!(pv_list_cache) as *mut c_void,
            b"pv_entry\0".as_ptr(),
            core::mem::size_of::<PvEntry>(),
            0,
            core::ptr::null(),
            0,
        );

        pmap_initialized = 1;
    }
}

// ───────────────────────── Trivial entry points ────────────────────

/// `pmap_pageable(...)` (pmap.c:2818) — advisory, empty.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_pageable(
    _pmap: pmap_t,
    _start: vm_offset_t,
    _end: vm_offset_t,
    _pageable: boolean_t,
) {
}

/// `pmap_update_interrupt()` (pmap.c:3241) — never called on UP.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_update_interrupt() {}

/// `pmap_reference(p)` (pmap.c:1558).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_reference(p: pmap_t) {
    if !p.is_null() {
        unsafe { (*p).ref_count += 1 };
    }
}

// ───────────────────────── pmap_map_bd ─────────────────────────────

/// `pmap_map_bd(virt, start, end, prot)` (pmap.c:569): back-door mapping of
/// kernel VM at init (e.g. devices outside the direct map).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_map_bd(
    mut virt: vm_offset_t,
    mut start: phys_addr_t,
    end: phys_addr_t,
    prot: vm_prot_t,
) -> vm_offset_t {
    let mut template =
        pa_to_pte(start) | INTEL_PTE_NCACHE | INTEL_PTE_WTHRU | INTEL_PTE_VALID;
    if (prot & VM_PROT_WRITE) != 0 {
        template |= INTEL_PTE_WRITE;
    }
    while start < end {
        let pte = pmap_pte(unsafe { kernel_pmap }, virt);
        if pte.is_null() {
            pmap_panic(b"pmap_map_bd: Invalid kernel address\n\0");
        }
        unsafe {
            *pte = template;
            invalidate_tlb(kernel_pmap, virt, virt + PAGE_SIZE);
        }
        template += PAGE_SIZE as pt_entry_t;
        virt += PAGE_SIZE;
        start += PAGE_SIZE as phys_addr_t;
    }
    virt
}

// ───────────────────────── pmap_create / destroy ───────────────────

/// `pmap_create(size)` (pmap.c:1330), non-PAE: clone the kernel page dir.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_create(size: vm_size_t) -> pmap_t {
    // A software-only map needs no real pmap.
    if size != 0 {
        return core::ptr::null_mut();
    }

    let p = unsafe { kmem_cache_alloc(core::ptr::addr_of_mut!(pmap_cache) as *mut c_void) }
        as pmap_t;
    if p.is_null() {
        return core::ptr::null_mut();
    }

    // One page directory (PDPNUM == 1), copied from the kernel's.
    let page_dir = unsafe { kmem_cache_alloc(core::ptr::addr_of_mut!(pd_cache) as *mut c_void) }
        as *mut pt_entry_t;
    if page_dir.is_null() {
        unsafe { kmem_cache_free(core::ptr::addr_of_mut!(pmap_cache) as *mut c_void, p as vm_offset_t) };
        return core::ptr::null_mut();
    }
    unsafe {
        memcpy(
            page_dir as *mut c_void,
            kernel_page_dir as *const c_void,
            INTEL_PGBYTES,
        );

        // LINUX_DEV: do not map the BIOS area in user tasks (pmap.c:1375).
        // VM_MIN_KERNEL_ADDRESS != 0, so clear the PDE for the BIOS window.
        *page_dir.add(lin2pdenum(LINEAR_MIN_KERNEL_ADDRESS - VM_MIN_KERNEL_ADDRESS)) = 0;

        (*p).dirbase = page_dir;
        (*p).ref_count = 1;
        (*p).cpus_using = 0;
        (*p).stats.resident_count = 0;
        (*p).stats.wired_count = 0;
    }
    p
}

/// `pmap_destroy(p)` (pmap.c:1486), non-PAE: free the page-table tree.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_destroy(p: pmap_t) {
    if p.is_null() {
        return;
    }
    let c = unsafe {
        (*p).ref_count -= 1;
        (*p).ref_count
    };
    if c != 0 {
        return; // still in use
    }

    unsafe {
        let pdebase = (*p).dirbase;
        // Free every user page table referenced by the directory.
        for l2i in 0..lin2pdenum(VM_MAX_USER_ADDRESS) {
            let pde = *pdebase.add(l2i);
            if (pde & INTEL_PTE_VALID) == 0 {
                continue;
            }
            kmem_cache_free(
                core::ptr::addr_of_mut!(pt_cache) as *mut c_void,
                ptetokv(pde),
            );
        }
        kmem_cache_free(
            core::ptr::addr_of_mut!(pd_cache) as *mut c_void,
            pdebase as vm_offset_t,
        );
        kmem_cache_free(
            core::ptr::addr_of_mut!(pmap_cache) as *mut c_void,
            p as vm_offset_t,
        );
    }
}

// ───────────────────────── remove range / remove ───────────────────

/// `pmap_remove_range(pmap, va, spte, epte)` (pmap.c:1582).
fn pmap_remove_range(
    pmap: pmap_t,
    mut va: vm_offset_t,
    spte: *mut pt_entry_t,
    epte: *mut pt_entry_t,
) {
    let span = unsafe { epte.offset_from(spte) } as vm_offset_t;
    if pmap == unsafe { kernel_pmap }
        && (va < unsafe { kernel_virtual_start }
            || va + span * PAGE_SIZE > unsafe { kernel_virtual_end })
    {
        pmap_panic(b"pmap_remove_range falls in physical memory area!\0");
    }

    let mut num_removed: i32 = 0;
    let mut num_unwired: i32 = 0;

    let mut cpte = spte;
    // Mirrors the C `for (cpte = spte; cpte < epte; cpte += 1, va += PAGE_SIZE)`:
    // `va` tracks the virtual address of the page mapped by `cpte` and is
    // needed for the per-page pv-list lookup.
    while cpte < epte {
        let pte = unsafe { *cpte };
        if pte == 0 {
            cpte = unsafe { cpte.add(1) };
            va += PAGE_SIZE;
            continue;
        }
        let pa = pte_to_pa(pte);
        num_removed += 1;
        if (pte & INTEL_PTE_WIRED) != 0 {
            num_unwired += 1;
        }

        if !valid_page(pa) {
            // Outside managed physical memory: just clear the PTE.
            unsafe { *cpte = 0 };
            cpte = unsafe { cpte.add(1) };
            va += PAGE_SIZE;
            continue;
        }

        let pai = pa_index(pa);
        // LOCK_PVH is a UP no-op.

        // Collect mod/ref bits and clear the PTE.
        unsafe {
            *pmap_phys_attributes.add(pai) |=
                (*cpte & (PHYS_MODIFIED | PHYS_REFERENCED)) as u8;
            *cpte = 0;
        }

        // Remove this mapping from the pv list for the page.
        unsafe {
            let pv_h = pai_to_pvh(pai);
            if (*pv_h).pmap.is_null() {
                pmap_panic(b"pmap_remove: null pv_list!\0");
            }
            if (*pv_h).va == va && (*pv_h).pmap == pmap {
                // Head is the entry: pull up the next one.
                let cur = (*pv_h).next;
                if !cur.is_null() {
                    *pv_h = PvEntry { next: (*cur).next, pmap: (*cur).pmap, va: (*cur).va };
                    pv_free(cur);
                } else {
                    (*pv_h).pmap = core::ptr::null_mut();
                }
            } else {
                let mut prev = pv_h;
                let mut cur = (*pv_h).next;
                loop {
                    if cur.is_null() {
                        pmap_panic(b"pmap-remove: mapping not in pv_list!\0");
                    }
                    if (*cur).va == va && (*cur).pmap == pmap {
                        break;
                    }
                    prev = cur;
                    cur = (*cur).next;
                }
                (*prev).next = (*cur).next;
                pv_free(cur);
            }
        }
        cpte = unsafe { cpte.add(1) };
        va += PAGE_SIZE;
    }

    unsafe {
        (*pmap).stats.resident_count -= num_removed;
        (*pmap).stats.wired_count -= num_unwired;
    }
}

/// `pmap_remove(map, s, e)` (pmap.c:1745).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_remove(map: pmap_t, mut s: vm_offset_t, e: vm_offset_t) {
    if map.is_null() {
        return;
    }
    let start = s;
    while s < e {
        let pde = pmap_pde(map, s);
        let mut l = (s + PDE_MAPPED_SIZE) & !(PDE_MAPPED_SIZE - 1);
        if l > e || l < s {
            l = e;
        }
        if !pde.is_null() && (unsafe { *pde } & INTEL_PTE_VALID) != 0 {
            let base = ptetokv(unsafe { *pde }) as *mut pt_entry_t;
            let spte = unsafe { base.add(ptenum(s)) };
            let epte = unsafe { spte.add(intel_btop(l - s)) };
            pmap_remove_range(map, s, spte, epte);
        }
        s = l;
    }
    unsafe { pmap_update_tlbs(map, start, e) };
}

// ───────────────────────── page_protect / protect ──────────────────

/// `pmap_page_protect(phys, prot)` (pmap.c:1786): lower permissions on all
/// mappings of a physical page (or remove them).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_page_protect(phys: phys_addr_t, prot: vm_prot_t) {
    if !valid_page(phys) {
        return;
    }
    let remove = if prot == VM_PROT_READ || prot == (VM_PROT_READ | VM_PROT_EXECUTE) {
        false
    } else if prot == VM_PROT_ALL {
        return; // nothing to do
    } else {
        true
    };

    // PMAP_WRITE_LOCK is a UP no-op.
    let pai = pa_index(phys);
    let pv_h = pai_to_pvh(pai);

    unsafe {
        if (*pv_h).pmap.is_null() {
            return;
        }
        let mut prev = pv_h;
        let mut pv_e = pv_h;
        loop {
            let pmap = (*pv_e).pmap;
            let va = (*pv_e).va;
            let pte = pmap_pte(pmap, va);

            if remove || pmap == kernel_pmap {
                if (*pte & INTEL_PTE_WIRED) != 0 {
                    (*pmap).stats.wired_count -= 1;
                }
                *pmap_phys_attributes.add(pai) |=
                    (*pte & (PHYS_MODIFIED | PHYS_REFERENCED)) as u8;
                *pte = 0;
                (*pmap).stats.resident_count -= 1;

                if pv_e == pv_h {
                    (*pv_h).pmap = core::ptr::null_mut(); // fix up head later
                } else {
                    (*prev).next = (*pv_e).next;
                    pv_free(pv_e);
                }
            } else {
                // Write-protect.
                *pte &= !INTEL_PTE_WRITE;
                prev = pv_e;
            }
            pmap_update_tlbs(pmap, va, va + PAGE_SIZE);

            pv_e = (*prev).next;
            if pv_e.is_null() {
                break;
            }
        }

        // If we cleared the head, pull up the next entry.
        if (*pv_h).pmap.is_null() {
            let next = (*pv_h).next;
            if !next.is_null() {
                *pv_h = PvEntry { next: (*next).next, pmap: (*next).pmap, va: (*next).va };
                pv_free(next);
            }
        }
    }
}

/// `pmap_protect(map, s, e, prot)` (pmap.c:1951).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_protect(map: pmap_t, mut s: vm_offset_t, e: vm_offset_t, prot: vm_prot_t) {
    if map.is_null() {
        return;
    }
    if prot == VM_PROT_READ || prot == (VM_PROT_READ | VM_PROT_EXECUTE) {
        // Fall through: write-protect the range below.
    } else if prot == (VM_PROT_READ | VM_PROT_WRITE) || prot == VM_PROT_ALL {
        return; // nothing to do
    } else {
        pmap_remove(map, s, e);
        return;
    }

    let start = s;
    // SPLVM / simple_lock are UP no-ops.
    while s < e {
        let pde = pmap_pde(map, s);
        let mut l = (s + PDE_MAPPED_SIZE) & !(PDE_MAPPED_SIZE - 1);
        if l > e || l < s {
            l = e;
        }
        if !pde.is_null() && (unsafe { *pde } & INTEL_PTE_VALID) != 0 {
            let base = ptetokv(unsafe { *pde }) as *mut pt_entry_t;
            let mut spte = unsafe { base.add(ptenum(s)) };
            let epte = unsafe { base.add(ptenum(s) + intel_btop(l - s)) };
            while spte < epte {
                if unsafe { *spte } & INTEL_PTE_VALID != 0 {
                    unsafe { *spte &= !INTEL_PTE_WRITE };
                }
                spte = unsafe { spte.add(1) };
            }
        }
        s = l;
    }
    unsafe { pmap_update_tlbs(map, start, e) };
}

// ───────────────────────── pmap_expand ─────────────────────────────

/// `pmap_expand(pmap, v)` (pmap.c:2149), non-PAE: ensure the page table for
/// `v` exists, allocating it if needed, and return the PTE pointer.
fn pmap_expand(pmap: pmap_t, v: vm_offset_t) -> *mut pt_entry_t {
    loop {
        let pte = pmap_pte(pmap, v);
        if !pte.is_null() {
            return pte;
        }
        // Need a new page-table page.
        if pmap == unsafe { kernel_pmap } {
            pmap_panic(b"pmap_expand kernel pmap\0");
        }

        // PMAP_READ_UNLOCK / RELOCK are UP no-ops.
        let ptp = loop {
            let p = unsafe { kmem_cache_alloc(core::ptr::addr_of_mut!(pt_cache) as *mut c_void) };
            if p != 0 {
                break p;
            }
            unsafe { vm_page_wait(core::ptr::null()) };
        };
        unsafe { memset(ptp as *mut c_void, 0, PAGE_SIZE) };

        // Did another thread already populate it?  (Can't on UP, but mirror C.)
        if !pmap_pte(pmap, v).is_null() {
            unsafe {
                kmem_cache_free(core::ptr::addr_of_mut!(pt_cache) as *mut c_void, ptp)
            };
            continue;
        }

        // Install the new page table in the directory.
        let pdp = pmap_pde(pmap, v);
        unsafe {
            *pdp = pa_to_pte(_kvtophys(ptp))
                | INTEL_PTE_VALID
                | INTEL_PTE_USER
                | INTEL_PTE_WRITE;
        }
    }
}

// ───────────────────────── pmap_enter ──────────────────────────────

/// Build the PTE template shared by the two `pmap_enter` paths
/// (pmap.c:2253-2262 / 2365-2374).  `INTEL_PTE_MOD` is OR-ed in by the
/// caller on the remap path.
#[inline]
fn pmap_enter_template(
    pmap: pmap_t,
    pa: phys_addr_t,
    prot: vm_prot_t,
    wired: boolean_t,
    is_physmem: bool,
) -> pt_entry_t {
    let mut t = pa_to_pte(pa) | INTEL_PTE_VALID;
    if pmap != unsafe { kernel_pmap } {
        t |= INTEL_PTE_USER;
    }
    if (prot & VM_PROT_WRITE) != 0 {
        t |= INTEL_PTE_WRITE;
    }
    if unsafe { machine_slot[0].cpu_type } >= CPU_TYPE_I486 && !is_physmem {
        t |= INTEL_PTE_NCACHE | INTEL_PTE_WTHRU;
    }
    if wired != 0 {
        t |= INTEL_PTE_WIRED;
    }
    t
}

/// `pmap_enter(pmap, v, pa, prot, wired)` (pmap.c:2172).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_enter(
    pmap: pmap_t,
    v: vm_offset_t,
    pa: phys_addr_t,
    prot: vm_prot_t,
    wired: boolean_t,
) {
    if pmap.is_null() {
        return;
    }
    if pmap == unsafe { kernel_pmap }
        && (v < unsafe { kernel_virtual_start } || v >= unsafe { kernel_virtual_end })
    {
        pmap_panic(b"pmap_enter falls in physical memory area!\0");
    }

    let mut pv_e: *mut PvEntry = core::ptr::null_mut();
    loop {
        let pte = pmap_expand(pmap, v);

        let is_physmem = if unsafe { vm_page_ready() } != 0 {
            !unsafe { vm_page_lookup_pa(pa) }.is_null()
        } else {
            pa < unsafe { biosmem_directmap_end() }
        };

        let old_pa = pte_to_pa(unsafe { *pte });

        if unsafe { *pte } != 0 && old_pa == pa {
            // Same page already mapped here: maybe change wiring/protection.
            let cur = unsafe { *pte };
            if wired != 0 && (cur & INTEL_PTE_WIRED) == 0 {
                unsafe { (*pmap).stats.wired_count += 1 };
            } else if wired == 0 && (cur & INTEL_PTE_WIRED) != 0 {
                unsafe { (*pmap).stats.wired_count -= 1 };
            }
            let mut t = pmap_enter_template(pmap, pa, prot, wired, is_physmem);
            if (cur & INTEL_PTE_MOD) != 0 {
                t |= INTEL_PTE_MOD;
            }
            unsafe {
                *pte = t;
                pmap_update_tlbs(pmap, v, v + PAGE_SIZE);
            }
        } else {
            // Remove an existing different mapping first.
            if unsafe { *pte } != 0 {
                pmap_remove_range(pmap, v, pte, unsafe { pte.add(PTES_PER_VM_PAGE) });
                unsafe { pmap_update_tlbs(pmap, v, v + PAGE_SIZE) };
            }

            if valid_page(pa) {
                let pai = pa_index(pa);
                let pv_h = pai_to_pvh(pai);
                unsafe {
                    if (*pv_h).pmap.is_null() {
                        (*pv_h).va = v;
                        (*pv_h).pmap = pmap;
                        (*pv_h).next = core::ptr::null_mut();
                    } else {
                        if pv_e.is_null() {
                            pv_e = pv_alloc();
                            if pv_e.is_null() {
                                // Refill from the slab cache and retry.
                                pv_e = kmem_cache_alloc(
                                    core::ptr::addr_of_mut!(pv_list_cache) as *mut c_void,
                                ) as *mut PvEntry;
                                continue;
                            }
                        }
                        (*pv_e).va = v;
                        (*pv_e).pmap = pmap;
                        (*pv_e).next = (*pv_h).next;
                        (*pv_h).next = pv_e;
                        pv_e = core::ptr::null_mut();
                    }
                }
            }

            unsafe {
                (*pmap).stats.resident_count += 1;
                if wired != 0 {
                    (*pmap).stats.wired_count += 1;
                }
                *pte = pmap_enter_template(pmap, pa, prot, wired, is_physmem);
            }
        }
        break;
    }

    if !pv_e.is_null() {
        unsafe { pv_free(pv_e) };
    }
}

// ───────────────────────── wiring / extract / collect ──────────────

/// `pmap_change_wiring(map, v, wired)` (pmap.c:2402).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_change_wiring(map: pmap_t, v: vm_offset_t, wired: boolean_t) {
    let pte = pmap_pte(map, v);
    if pte.is_null() {
        pmap_panic(b"pmap_change_wiring: pte missing\0");
    }
    let cur = unsafe { *pte };
    if wired != 0 && (cur & INTEL_PTE_WIRED) == 0 {
        unsafe {
            (*map).stats.wired_count += 1;
            *pte |= INTEL_PTE_WIRED;
        }
    } else if wired == 0 && (cur & INTEL_PTE_WIRED) != 0 {
        unsafe {
            (*map).stats.wired_count -= 1;
            *pte &= !INTEL_PTE_WIRED;
        }
    }
}

/// `pmap_extract(pmap, va)` (pmap.c:2457).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_extract(pmap: pmap_t, va: vm_offset_t) -> phys_addr_t {
    let pte = pmap_pte(pmap, va);
    if pte.is_null() {
        return 0;
    }
    let v = unsafe { *pte };
    if (v & INTEL_PTE_VALID) == 0 {
        0
    } else {
        pte_to_pa(v) + (va & INTEL_OFFMASK) as phys_addr_t
    }
}

/// `pmap_collect(p)` (pmap.c:2507), non-PAE: free unused user page tables.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_collect(p: pmap_t) {
    if p.is_null() || p == unsafe { kernel_pmap } {
        return;
    }
    unsafe {
        let pdebase = (*p).dirbase;
        for l2i in 0..lin2pdenum(VM_MAX_USER_ADDRESS) {
            let pde = *pdebase.add(l2i);
            if (pde & INTEL_PTE_VALID) == 0 {
                continue;
            }
            let pa = pte_to_pa(pde);
            let ptp = phystokv(pa) as *mut pt_entry_t;
            let eptp = ptp.add(NPTES);

            // Skip if any mapping in this table is wired.
            let mut wired = false;
            let mut q = ptp;
            while q < eptp {
                if (*q & INTEL_PTE_WIRED) != 0 {
                    wired = true;
                    break;
                }
                q = q.add(1);
            }
            if wired {
                continue;
            }

            // Remove all VAs mapped by this table, then drop the PDE.
            let va = pdenum2lin(l2i);
            let va = if p == kernel_pmap { va } else { va }; // lintokv == identity for user
            pmap_remove_range(p, va, ptp, eptp);
            *pdebase.add(l2i) = 0;
            kmem_cache_free(core::ptr::addr_of_mut!(pt_cache) as *mut c_void, ptetokv(pde));
        }
        pmap_update_tlbs(p, 0, VM_MAX_USER_ADDRESS);
    }
}

// ───────────────────────── phys attribute (mod/ref) ────────────────

/// `phys_attribute_clear(phys, bits)` (pmap.c:2830).
fn phys_attribute_clear(phys: phys_addr_t, bits: pt_entry_t) {
    if !valid_page(phys) {
        return;
    }
    let pai = pa_index(phys);
    let pv_h = pai_to_pvh(pai);
    unsafe {
        if !(*pv_h).pmap.is_null() {
            let mut pv_e = pv_h;
            while !pv_e.is_null() {
                let pmap = (*pv_e).pmap;
                let va = (*pv_e).va;
                let pte = pmap_pte(pmap, va);
                *pte &= !bits;
                pmap_update_tlbs(pmap, va, va + PAGE_SIZE);
                pv_e = (*pv_e).next;
            }
        }
        *pmap_phys_attributes.add(pai) &= !(bits as u8);
    }
}

/// `phys_attribute_test(phys, bits)` (pmap.c:2914).
fn phys_attribute_test(phys: phys_addr_t, bits: pt_entry_t) -> boolean_t {
    if !valid_page(phys) {
        return 0;
    }
    let pai = pa_index(phys);
    let pv_h = pai_to_pvh(pai);
    unsafe {
        if (*pmap_phys_attributes.add(pai) & bits as u8) != 0 {
            return 1;
        }
        if !(*pv_h).pmap.is_null() {
            let mut pv_e = pv_h;
            while !pv_e.is_null() {
                let pmap = (*pv_e).pmap;
                let va = (*pv_e).va;
                let pte = pmap_pte(pmap, va);
                if (*pte & bits) != 0 {
                    return 1;
                }
                pv_e = (*pv_e).next;
            }
        }
    }
    0
}

/// `pmap_clear_modify(phys)` (pmap.c:3004).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_clear_modify(phys: phys_addr_t) {
    phys_attribute_clear(phys, PHYS_MODIFIED);
}

/// `pmap_is_modified(phys)` (pmap.c:3016).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_is_modified(phys: phys_addr_t) -> boolean_t {
    phys_attribute_test(phys, PHYS_MODIFIED)
}

/// `pmap_clear_reference(phys)` (pmap.c:3027).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_clear_reference(phys: phys_addr_t) {
    phys_attribute_clear(phys, PHYS_REFERENCED);
}

/// `pmap_is_referenced(phys)` (pmap.c:3039).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_is_referenced(phys: phys_addr_t) -> boolean_t {
    phys_attribute_test(phys, PHYS_REFERENCED)
}

// ───────────────────────── boot-time mapping fixups ────────────────

/// `pmap_unmap_page_zero()` (pmap.c:3249): unmap VA 0 to trap NULL derefs.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_unmap_page_zero() {
    unsafe {
        printf(b"Unmapping the zero page.  Some BIOS functions may not be working any more.\n\0".as_ptr());
        let pte = pmap_pte(kernel_pmap, 0);
        if pte.is_null() {
            return;
        }
        *pte = 0;
        invalidate_tlb(kernel_pmap, 0, PAGE_SIZE);
    }
}

/// `pmap_make_temporary_mapping()` (pmap.c:3269).
///
/// Installs a temporary identity-ish direct mapping between physical memory
/// and low linear memory until the new kernel segments take over.  Active
/// because `INIT_VM_MIN_KERNEL_ADDRESS (0) != LINEAR_MIN_KERNEL_ADDRESS
/// (0xC0000000)`.  Also (LINUX_DEV) maps the BIOS window.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_make_temporary_mapping() {
    unsafe {
        // delta = |INIT_VM_MIN_KERNEL_ADDRESS - LINEAR_MIN_KERNEL_ADDRESS|
        let mut delta = INIT_VM_MIN_KERNEL_ADDRESS.wrapping_sub(LINEAR_MIN_KERNEL_ADDRESS);
        if (delta.wrapping_neg()) < delta {
            delta = delta.wrapping_neg();
        }
        let nb_direct = delta >> PDESHIFT;
        let dst0 = lin2pdenum_cont(INIT_VM_MIN_KERNEL_ADDRESS);
        let src0 = lin2pdenum_cont(LINEAR_MIN_KERNEL_ADDRESS);
        for i in 0..nb_direct {
            *kernel_page_dir.add(dst0 + i) = *kernel_page_dir.add(src0 + i);
        }

        // LINUX_DEV, VM_MIN_KERNEL_ADDRESS != 0: keep BIOS memory mapped at
        // 0xc0000 & co for BIOS accesses.
        *kernel_page_dir.add(lin2pdenum_cont(LINEAR_MIN_KERNEL_ADDRESS - VM_MIN_KERNEL_ADDRESS)) =
            *kernel_page_dir.add(lin2pdenum_cont(LINEAR_MIN_KERNEL_ADDRESS));
    }
}

/// `pmap_set_page_dir()` (pmap.c:3310), non-PAE: load `%cr3` with the kernel
/// page directory.
#[unsafe(no_mangle)]
pub extern "C" fn pmap_set_page_dir() {
    unsafe { set_cr3(_kvtophys(kernel_page_dir as vm_offset_t)) };
}

/// `pmap_remove_temporary_mapping()` (pmap.c:3329).
#[unsafe(no_mangle)]
pub extern "C" fn pmap_remove_temporary_mapping() {
    unsafe {
        let mut delta = INIT_VM_MIN_KERNEL_ADDRESS.wrapping_sub(LINEAR_MIN_KERNEL_ADDRESS);
        if (delta.wrapping_neg()) < delta {
            delta = delta.wrapping_neg();
        }
        let nb_direct = delta >> PDESHIFT;
        let dst0 = lin2pdenum_cont(INIT_VM_MIN_KERNEL_ADDRESS);
        for i in 0..nb_direct {
            // Get rid of the temporary direct mapping and flush the TLB.
            *kernel_page_dir.add(dst0 + i) = 0;
        }

        // LINUX_DEV: keep BIOS memory mapped.
        *kernel_page_dir.add(lin2pdenum_cont(LINEAR_MIN_KERNEL_ADDRESS - VM_MIN_KERNEL_ADDRESS)) =
            *kernel_page_dir.add(lin2pdenum_cont(LINEAR_MIN_KERNEL_ADDRESS));

        flush_tlb();
    }
}
