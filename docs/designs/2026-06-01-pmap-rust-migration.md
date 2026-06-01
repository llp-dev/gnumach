# PMAP Module: Idiomatic Rust Migration

**Date:** 2026-06-01  
**Status:** Design (awaiting plan)

---

## Problem Statement

The GNU Mach PMAP module (`i386/intel/pmap.c`, 3,367 lines) implements x86 physical-to-virtual memory mapping with heavy conditional compilation for 32-bit non-PAE, 32-bit PAE, x86_64, Xen PV, and SMP variants — all in a single C file. This makes the module difficult to refactor, test in isolation, and extend (e.g., for aarch64). Migrating to idiomatic Rust improves safety through type-enforced invariants, enables unit testing of pure logic, and positions the kernel for future architecture targets.

---

## Design Overview

A single Rust `staticlib` crate (`rust/pmap/`) replaces `i386/intel/pmap.c` (except bootstrap) and `i386/intel/pmap.h`. The crate compiles to `libpmap.a` and links into the existing kernel binary via the Autotools build system. All C-visible functions are exposed through `#[no_mangle] extern "C"` thin wrappers in `ffi.rs`. The internal Rust API uses idiomatic types: `Option<T>` for nullable results, `Result<T, E>` for fallible operations, lock guards with `Drop` for scoped locking, and `#[repr(C)]` structs for shared state with C.

### Key principles

- **Isolation:** `ffi.rs` is the sole unsafe boundary between C and Rust. Internal Rust code operates on safe references.
- **Feature flags replace `#ifdef`**: `pae`, `x86_64`, `smp`, `xen`, `xen_hyp`, `pseudo_phys`, `kdb`, `debug_pte` map directly to Rust `#[cfg(feature = "...")]`.
- **Bootstrap stays in C:** `pmap_bootstrap()` and related early-init functions remain in a minimal `pmap_bootstrap.c` — they run before Rust's runtime is viable.
- **No allocation in Rust bootstrap path:** All slab caches (`kmem_cache`) are created in C's `pmap_init()`; Rust calls `extern "C"` allocator functions.

---

## Component Breakdown

### Cargo crate structure

```
rust/pmap/
├── Cargo.toml
├── build.rs                     # Invokes bindgen for kernel C types
├── src/
│   ├── lib.rs                   # Crate root, #[no_std], panic_handler, module declarations
│   ├── bindings.rs              # (auto-generated) bindgen output
│   ├── types.rs                 # #[repr(C)] Pmap, Pte, PvEntry, PmapStatistics, SimpleLock, globals
│   ├── locking.rs               # PmapReadGuard, PmapWriteGuard, PvLockGuard, SplGuard
│   ├── lifecycle.rs             # pmap_create, pmap_destroy, pmap_reference
│   ├── mapping.rs               # pmap_enter, pmap_remove, pmap_extract, pmap_change_wiring, pmap_pageable, pmap_map_bd
│   ├── protection.rs            # pmap_protect, pmap_page_protect
│   ├── pv_list.rs               # pv_entry alloc/free, pv_head_table, lock bits, valid_page
│   ├── expand.rs                # pmap_expand_level, pmap_expand, pmap_pte, pmap_pde, pmap_l4base
│   ├── page_attr.rs             # pmap_clear_modify, pmap_is_modified, pmap_clear_reference, pmap_is_referenced
│   ├── phys_ops.rs              # pmap_zero_page, pmap_copy_page, copy_to_phys, copy_from_phys, kvtophys
│   ├── mapwindow.rs             # pmap_get_mapwindow, pmap_put_mapwindow
│   ├── activation.rs            # pmap_activate, pmap_deactivate (from macros → functions)
│   ├── collect.rs               # pmap_collect
│   ├── whatis.rs                # pmap_whatis (#[cfg(feature = "kdb")])
│   ├── smp.rs                   # signal_cpus, process_pmap_updates, pmap_update_interrupt (#[cfg(feature = "smp")])
│   ├── xen.rs                   # pmap_set_page_readwrite/readonly, pmap_map_mfn, bootstrap helpers (#[cfg(feature = "xen")])
│   └── ffi.rs                   # #[no_mangle] pub extern "C" wrappers for every public function
```

### Module responsibilities

| Module | Purpose | Exports |
|--------|---------|---------|
| `types` | All `#[repr(C)]` structs, global `static mut` declarations, type aliases | `Pmap`, `PvEntry`, `Pte`, `PmapMapwindow`, `CpuSet`, `SimpleLock`, type aliases |
| `locking` | Scoped lock guards with `Drop` that enforce SPLVM + lock protocol | `PmapReadGuard`, `PmapWriteGuard`, `PvLockGuard`, `SplGuard` |
| `lifecycle` | Create/destroy/reference pmap structs and page table hierarchies | `pmap_create()`, `pmap_destroy()`, `pmap_reference()` |
| `mapping` | Core virtual→physical mapping operations | `pmap_enter()`, `pmap_remove()`, `pmap_extract()`, `pmap_change_wiring()`, `pmap_pageable()`, `pmap_map_bd()` |
| `protection` | Address-range and per-page protection changes | `pmap_protect()`, `pmap_page_protect()` |
| `pv_list` | Physical→virtual reverse mapping table management | `pv_alloc()`, `pv_free()`, `lock_pvh()`, `unlock_pvh()`, `valid_page()` |
| `expand` | Page table walking and on-demand allocation of intermediate page tables | `pmap_pte()`, `pmap_expand()`, `pmap_expand_level()`, `pmap_pde()`, `pmap_l4base()` |
| `page_attr` | Modified/reference bit tracking across all mappings of a physical page | `pmap_clear_modify()`, `pmap_is_modified()`, `pmap_clear_reference()`, `pmap_is_referenced()` |
| `phys_ops` | Zero/copy physical pages, kernel virtual↔physical translation | `pmap_zero_page()`, `pmap_copy_page()`, `kvtophys()`, `copy_to_phys()`, `copy_from_phys()` |
| `mapwindow` | Temporary kernel window mappings for pages outside direct map | `pmap_get_mapwindow()`, `pmap_put_mapwindow()` |
| `activation` | Address space switching (replaces C macros with proper functions) | `pmap_activate()`, `pmap_deactivate()` |
| `collect` | Garbage-collect unused page-table pages from user pmaps | `pmap_collect()` |
| `whatis` | Kernel debugger address inspection | `pmap_whatis()` (#[cfg(feature = "kdb")]) |
| `smp` | Multi-CPU TLB shootdown via IPI | `signal_cpus()`, `process_pmap_updates()`, `pmap_update_interrupt()` (#[cfg(feature = "smp")]) |
| `xen` | Xen PV page table pinning, readwrite/readonly transitions, MFN mapping | `pmap_set_page_readwrite()`, `pmap_set_page_readonly()`, `pmap_map_mfn()` (#[cfg(feature = "xen")]) |
| `ffi` | C-compatible `#[no_mangle] extern "C"` trampolines | All `pmap_*()` symbols expected by the kernel linker |

### Internal helper functions (pub(crate) or private)

These live in the module that uses them, never exported:

- `pmap_remove_range()` — in `mapping.rs`
- `phys_attribute_clear()` / `phys_attribute_test()` — in `page_attr.rs`
- `pmap_expand_level()` — in `expand.rs`
- `ptep_check()` — in `expand.rs` (#[cfg(feature = "debug_pte")])

### Static globals owned by C bootstrap

Declared as `extern "C"` in `types.rs`, wrapped with safe accessors:

```rust
extern "C" {
    static mut kernel_pmap: *mut Pmap;
    static mut kernel_page_dir: *mut Pte;
    static mut kernel_virtual_start: VmOffset;
    static mut kernel_virtual_end: VmOffset;
    static mut pv_lock_table: *mut u8;
    static mut pv_head_table: *mut PvEntry;
    static mut pmap_phys_attributes: *mut u8;
    static mut pmap_initialized: c_int;
    static mut pmap_system_lock: LockData;         // #[cfg(feature = "smp")]
    static mut cpus_active: CpuSet;                 // #[cfg(feature = "smp")]
    static mut cpus_idle: CpuSet;                   // #[cfg(feature = "smp")]
    static mut cpu_update_needed: [c_int; NCPUS];   // #[cfg(feature = "smp")]
}
```

---

## Data Flow

### Normal operation flow

```
C caller (e.g., vm_map.c)
    │
    ▼
ffi.rs: pmap_enter(*mut Pmap, ...)     ← unsafe boundary
    │  converts c_int → bool, *mut T → &T
    ▼
mapping.rs: pmap_enter(&Pmap, ...)     ← safe Rust
    │
    ├── locking::PmapReadGuard::acquire()  → SPLVM + lock_read + simple_lock
    ├── expand::pmap_expand()              → allocates page tables if needed
    ├── pv_list::lock_pvh()                → locks pv entry
    ├── raw PTE write (unsafe)             → writes page table entry
    ├── pv_list::unlock_pvh()              → unlocks pv entry
    └── Drop(PmapReadGuard)                → simple_unlock + lock_read_done + SPLX
```

### Bootstrap handoff flow

```
C startup (startup.c / cstartup.c)
    │
    ├── pmap_bootstrap()          ← C (allocates kernel_page_dir, direct-maps RAM)
    ├── pmap_set_page_dir()       ← C (sets CR3)
    ├── ... kernel VM init ...
    ├── pmap_init()               ← C (creates slab caches, pv_head_table)
    │       └── sets pmap_initialized = TRUE
    │
    ▼
All subsequent pmap_*() calls     ← Rust (reads pmap_initialized, uses slab caches via FFI)
```

### SMP TLB shootdown flow

```
CPU A: pmap_remove(kernel_pmap, start, end)
    │
    ├── PMAP_UPDATE_TLBS(pmap, start, end)
    │       ├── finds other CPUs in pmap->cpus_using
    │       ├── signal_cpus(users, pmap, start, end)
    │       │       ├── queues update items on per-CPU lists
    │       │       └── sends IPI to each target CPU
    │       ├── waits for CPUs to leave pmap->cpus_using
    │       └── INVALIDATE_TLB on local CPU
    │
    ▼
CPU B: pmap_update_interrupt()    ← IPI handler
    ├── processes queued updates
    ├── INVALIDATE_TLB for matching entries
    └── clears update_needed flag
```

---

## Feature Flags & Variant Configuration

### Cargo.toml features

```toml
[features]
default = []
pae = []                        # 3+ level page tables, pdpbase
x86_64 = ["pae"]                # 4-level tables, l4base, L4SHIFT
smp = []                        # Multi-CPU TLB shootdown, pmap_system_lock
xen = []                        # Xen PV: page pinning, hypercall PTE writes
xen_hyp = ["xen"]               # Xen with separate user L4 (MACH_HYP)
pseudo_phys = []                # Address translation via pa_to_ma
kdb = []                        # pmap_whatis() debug support
debug_pte = []                  # ptep_check() PTE page auditing
```

### Feature flag table

| C conditional | Rust feature | Controls |
|---|---|---|
| `!PAE` | (default) | 2-level paging: `dirbase`, 10-bit PDE, 10-bit PTE |
| `PAE` (32-bit) | `pae` | 3-level: `pdpbase`, PDPT+PDE+PTE, `pmap_ptp()` |
| `__x86_64__` + `PAE` | `x86_64` | 4-level: `l4base`, L4+PDPT+PDE+PTE, `pmap_l4base()`, sign-extended addresses |
| `NCPUS > 1` | `smp` | `pmap_system_lock` (r/w), SPLVM cpus_active protocol, `signal_cpus()` |
| `MACH_PV_PAGETABLES` | `xen` | `WRITE_PTE` → hypercall, `INVALIDATE_TLB` → `flush_tlb`, page pinning |
| `MACH_HYP` | `xen_hyp` | `user_l4base`, `user_pdpbase`, `hyp_set_user_cr3()` |
| `MACH_PSEUDO_PHYS` | `pseudo_phys` | `WRITE_PTE(pa_to_ma(...))` |
| `MACH_KDB` | `kdb` | `pmap_whatis()` |
| `DEBUG_PTE_PAGE` | `debug_pte` | `ptep_check()` |

### Feature combinations

Valid combinations enforced by feature hierarchy and compile-time checks:

- `x86_64` implies `pae`
- `xen_hyp` implies `xen`
- `xen_hyp` requires `x86_64` (compile-time `compile_error!`)
- All other features are orthogonal

---

## Locking Design

### Guard types

```rust
/// Holds SPLVM level + removes CPU from cpus_active set.
/// Restores on Drop.
pub struct SplGuard { prev_spl: c_int }

/// Read lock on pmap_system_lock + simple_lock on one Pmap.
/// Enables pmap-based operations (pmap_enter, pmap_remove, ...).
pub struct PmapReadGuard<'a> { _spl: SplGuard, pmap: &'a Pmap }

/// Exclusive write lock on pmap_system_lock.
/// Enables pv_list-based operations (pmap_remove_all, pmap_copy_on_write, ...).
pub struct PmapWriteGuard { _spl: SplGuard }

/// Bit lock on a pv_head_table entry.
pub struct PvLockGuard { index: usize }
```

### Lock protocol enforcement via types

Functions declare their locking requirement in their signature:

```rust
// pmap-based operation: needs read lock
pub fn pmap_enter(pmap: &Pmap, va: VmOffset, pa: PhysAddr, prot: VmProt, wired: bool) {
    let _guard = PmapReadGuard::acquire(pmap);
    // ...
} // _guard drops here → unlock

// pv_list-based operation: needs write lock
pub fn pmap_page_protect(phys: PhysAddr, prot: VmProt) {
    let _guard = PmapWriteGuard::acquire();
    // ...
}

// No lock needed (pure lookup)
pub fn pmap_extract(pmap: &Pmap, va: VmOffset) -> Option<PhysAddr> { ... }
```

### SMP vs UP differences

On UP (`#[cfg(not(feature = "smp"))]`):
- `SplGuard` is a zero-sized no-op (SPLVM and cpus_active are meaningless)
- `PmapReadGuard` only locks the pmap's simple_lock (no pmap_system_lock)
- `PmapWriteGuard` is a zero-sized no-op
- `PvLockGuard` is a zero-sized no-op (no concurrent access on UP)

---

## Error Handling Strategy

| Scenario | C pattern | Rust equivalent | FFI mapping |
|----------|-----------|-----------------|-------------|
| Invariant violation | `panic("...")` | `panic!("...")` → abort via `#[panic_handler]` | Same behavior |
| Null/no mapping | Return 0 or NULL | `Option<T>` where `None` = not found | `unwrap_or(0)` / `unwrap_or(null())` |
| Allocation failure | Return `KERN_RESOURCE_SHORTAGE` | `Result<T, KernReturn>` | Map `Err(KERN_RESOURCE_SHORTAGE)` to return value |
| Boolean query | Return FALSE/TRUE (int) | `bool` | `bool as c_int` |
| Infallible operation | `void` return | No `Result`, may `panic!` on OOM | `void` C function |

### panic_handler

```rust
// In lib.rs — called on any panic!()
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    extern "C" { fn panic(msg: *const u8, len: usize) -> !; }
    let msg = format_args!("{}", info); // or a simpler approach for no_std
    unsafe { panic(msg.as_ptr(), msg.len()); }
}
```

The kernel's existing `panic()` function terminates the system.

---

## FFI Layer

### Pattern

Every public C function has a thin `#[no_mangle] extern "C"` wrapper in `ffi.rs`. The wrapper:
1. Converts C types to Rust types (`*mut T` → `&T`, `c_int` → `bool`, etc.)
2. Calls the idiomatic Rust function
3. Converts the return value back to C types

```rust
// ffi.rs
#[no_mangle]
pub extern "C" fn pmap_enter(
    pmap: *mut Pmap,
    va: VmOffset,
    pa: PhysAddr,
    prot: VmProt,
    wired: c_int,
) {
    let pmap = unsafe { &*pmap };
    let wired = wired != 0;
    crate::mapping::pmap_enter(pmap, va, pa, prot, wired);
}

#[no_mangle]
pub extern "C" fn pmap_extract(pmap: *mut Pmap, va: VmOffset) -> PhysAddr {
    let pmap = unsafe { &*pmap };
    crate::mapping::pmap_extract(pmap, va).unwrap_or(0)
}
```

### C header compatibility

The existing `vm/pmap.h` and `i386/intel/pmap.h` remain as C header files. Their function declarations are unchanged — they declare the same symbols that `ffi.rs` exports. This ensures zero changes to C callers.

**Removed from headers:** None. All declarations are preserved.

**Changed in implementation:** `pmap.c` is replaced by `libpmap.a`. The macro functions (`pmap_kernel()`, `pmap_copy()`, `pmap_attribute()`, `pmap_resident_count()`, `pmap_phys_address()`, `pmap_phys_to_frame()`) remain as C macros in the header — they don't need Rust implementations because they're compile-time expansions.

**Activate/deactivate macros:** `PMAP_ACTIVATE_USER`, `PMAP_DEACTIVATE_USER`, `PMAP_ACTIVATE_KERNEL`, `PMAP_DEACTIVATE_KERNEL` remain as C macros (they contain inline assembly/locking that is tightly coupled to the C caller context). The underlying `pmap_activate()` and `pmap_deactivate()` functions exist in Rust but the macros mostly bypass them for performance.

---

## Build Integration

### Cargo.toml

```toml
[package]
name = "pmap"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib"]

[features]
default = []
pae = []
x86_64 = ["pae"]
smp = []
xen = []
xen_hyp = ["xen"]
pseudo_phys = []
kdb = []
debug_pte = []

[profile.release]
lto = true
opt-level = "s"
panic = "abort"
codegen-units = 1

[profile.dev]
panic = "abort"

[build-dependencies]
# bindgen for generating src/bindings.rs from kernel C headers
# (optional — can also use hand-written extern blocks)
```

### Autotools integration

In the relevant `Makefrag` file, replace:

```makefile
# Before
libkernel_a_SOURCES += i386/intel/pmap.c

# After
libkernel_a_SOURCES += i386/intel/pmap_bootstrap.c   # bootstrap-only C file
libkernel_a_LIBADD += $(RUST_PMAP_DIR)/target/x86_64-unknown-none/release/libpmap.a

# Build rule
$(RUST_PMAP_DIR)/target/x86_64-unknown-none/release/libpmap.a:
	cd $(RUST_PMAP_DIR) && \
	RUSTFLAGS="-C link-arg=-nostdlib -C relocation-model=static -C code-model=kernel" \
	cargo build --release --target x86_64-unknown-none $(PMAP_CARGO_FEATURES)

PMAP_CARGO_FEATURES = \
	$(if $(PAE),--features pae) \
	$(if $(SMP),--features smp) \
	$(if $(MACH_XEN),--features xen) \
	$(if $(MACH_HYP),--features xen_hyp)
```

### Target specification

A custom target JSON (`x86_64-gnumach.json`) will be needed:

```json
{
    "arch": "x86_64",
    "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
    "disable-redzone": true,
    "features": "-mmx,-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2",
    "linker": "ld",
    "linker-flavor": "ld.lld",
    "llvm-target": "x86_64-unknown-none",
    "max-atomic-width": 64,
    "os": "none",
    "panic-strategy": "abort",
    "position-independent-executables": false,
    "relocation-model": "static",
    "code-model": "kernel",
    "target-c-int-width": "32",
    "target-pointer-width": "64"
}
```

An equivalent i386 target spec will be needed for 32-bit builds.

---

## Testing Strategy

### Layer 1: Host-side unit tests (cargo test)

Pure computation tests — no hardware or kernel needed:

- PTE bit math: `pa_to_pte` ↔ `pte_to_pa` round-trip
- Address decomposition: `l4_index()`, `pdp_index()`, `pde_index()`, `pte_index()` for known addresses
- `pagenum2lin()` reconstruction (inverse of index decomposition)
- `lin2pdenum()`, `lin2pdenum_cont()` correctness for all page sizes
- PvEntry list operations with mock allocator
- Lock guard Drop behavior (doesn't panic, runs exactly once)
- Feature combination: `cargo test --all-features` compiles all variants

### Layer 2: QEMU integration tests

Existing `tests/` infrastructure (QEMU + minimal kernel boot) extended with:

1. **Smoke test:** Kernel boots with Rust PMAP, `pmap_initialized` is true
2. **Enter/extract round-trip:** Map a physical page, extract returns correct PA
3. **Remove semantics:** After `pmap_remove`, extract returns 0
4. **Protection enforcement:** Enter RW, protect to RO, write access faults
5. **Create/destroy:** `pmap_create()` + `pmap_enter()` + `pmap_destroy()` doesn't leak
6. **Modified/reference tracking:** Write through mapping sets modified bit
7. **Wired count:** `pmap_change_wiring()` increments/decrements correctly
8. **SMP** (if `smp`): Boot with `-smp 2`, verify TLB shootdown doesn't deadlock
9. **Stress:** Enter/remove/protect loop verifies slab stability

### Layer 3: Property-based tests (optional, host-side)

Random operation sequences verify invariants:

- After destroy(create()), resident_count == 0
- enter(p, v, pa) then extract(p, v) == pa (until remove or remap)
- Protection can only be lowered by protect, never raised

---

## Functions Not Migrated

### Retained in C (`i386/intel/pmap_bootstrap.c`, extracted from pmap.c)

| Function | Reason |
|----------|--------|
| `pmap_bootstrap()` | Runs before kernel VM, with paging off |
| `pmap_bootstrap_pae()` | PAE-specific bootstrap (called by pmap_bootstrap) |
| `pmap_bootstrap_xen()` | Xen-specific bootstrap (called by pmap_bootstrap) |
| `pmap_set_page_dir()` | Sets CR3/CR4 during boot transition |
| `pmap_unmap_page_zero()` | Unmaps page 0 after boot |
| `pmap_make_temporary_mapping()` | Temporary direct mapping during GDT/segment transition |
| `pmap_remove_temporary_mapping()` | Removes temporary mapping, flushes TLB |
| `pmap_clear_bootstrap_pagetable()` | Xen: unpins and frees bootstrap page table pages after real tables established |
| `pmap_init()` | Creates slab caches, allocates pv_head_table, lock bits, phys_attributes array |

### Already defined outside pmap.c (no migration needed)

| Function | Defined in | Notes |
|----------|-----------|-------|
| `pmap_grab_page()` | `i386/i386at/model_dep.c:685` | Physical page allocator for init |
| `pmap_steal_memory()` | `vm/vm_resident.c:233` | Uses pmap_virtual_space + pmap_enter |
| `pmap_zero_page()` | `i386/i386/phys.c:50` | Assembly-optimized page zeroing |
| `pmap_copy_page()` | `i386/i386/phys.c:75` | Assembly-optimized page copy |

### Compiled-out dead code in current pmap.c (ignored)

The following functions are inside `#if 0` blocks in pmap.c and are not compiled:
`pmap_copy()`, `pmap_activate()` (trivial wrapper), `pmap_deactivate()` (trivial wrapper), `pmap_kernel()` (trivial wrapper), `pmap_zero_page()` (C stub), `pmap_copy_page()` (C stub). Their real implementations are macros or assembly, listed above.

### Functions migrated to Rust (~38 functions)

All remaining active functions from `i386/intel/pmap.c`:

**Core mapping API:** `pmap_enter`, `pmap_remove`, `pmap_extract`, `pmap_protect`, `pmap_page_protect`, `pmap_change_wiring`, `pmap_pageable`, `pmap_map_bd`, `pmap_virtual_space`

**Lifecycle:** `pmap_create`, `pmap_destroy`, `pmap_reference`, `pmap_collect`

**Page attributes:** `pmap_clear_modify`, `pmap_is_modified`, `pmap_clear_reference`, `pmap_is_referenced`

**Physical ops:** `kvtophys`, `copy_to_phys`, `copy_from_phys`

**Map windows:** `pmap_get_mapwindow`, `pmap_put_mapwindow`

**Activation:** `pmap_activate`, `pmap_deactivate` (called by macros in the C header)

**Xen runtime:** `pmap_set_page_readwrite`, `pmap_set_page_readonly`, `pmap_set_page_readonly_init`, `pmap_map_mfn`, `pmap_page_table_page_alloc`, `pmap_page_table_page_dealloc`

**SMP** (optional): `signal_cpus`, `process_pmap_updates`, `pmap_update_interrupt`

**Debug** (optional): `pmap_whatis`, `ptep_check`

**Internal helpers** (static in C, pub(crate) in Rust): `pmap_pte`, `pmap_pde`, `pmap_l4base`, `pmap_ptp`, `pmap_remove_range`, `pmap_expand`, `pmap_expand_level`, `phys_attribute_clear`, `phys_attribute_test`, `valid_page`

### Headers retained unchanged

- `i386/intel/pmap.h` — full retention, all macros and type definitions
- `vm/pmap.h` — full retention, machine-independent interface

---

## Open Questions

1. **Panic handler coordination:** The `#[panic_handler]` in `libpmap.a` must not conflict with any other Rust code linked into the kernel. If future Rust kernel modules exist, the panic handler should be in a separate `kernel-panic` crate that all modules link against.

2. **i386 target spec:** A 32-bit target JSON is needed for non-x86_64 builds. The exact flags differ (no `code-model=kernel`, different `features` set).

3. **bindgen scope:** Should bindgen parse all kernel headers or just the types PMAP needs? Full parse is more complete but adds build-time dependency on the C toolchain.

4. **Optimization parity:** The C code uses inline assembly for `set_cr3()`, `flush_tlb()`, etc. These will be `extern "C"` calls to small C wrappers or inline asm in Rust (via `core::arch::asm!`). Performance parity must be verified.

5. **NCPUS constant:** `NCPUS` is currently a C preprocessor constant. In Rust, it must be either a `build.rs`-generated constant or passed via `RUSTFLAGS=--cfg ncpus=\"4\"`. Exact mechanism TBD.

---

## Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Single crate with feature-gated modules | Direct map from current C file, no premature abstraction |
| Variant handling | `#[cfg(feature = "...")]` | Closest to existing C preprocessor, minimal learning curve |
| Build integration | Cargo sub-project invoked from Autotools | Keeps existing build system, incremental adoption |
| Safety level | Unsafe internals wrapped in safe APIs | Kernel requires unsafe operations; contain them at module boundaries |
| C type bridging | bindgen-generated typed wrappers | Type safety between distinct C pointer types |
| Locking | RAII guards with Drop | Prevents unlock-forgetting bugs; zero-cost on UP |
| Bootstrap | Keep in C | Avoids boot-time Rust viability concerns |
| FFI pattern | Thin wrappers in ffi.rs | Single unsafe boundary; internal code is idiomatic Rust |
| Error handling | Option/Result for failures, panic for invariants | Idiomatic Rust, clear failure semantics at FFI boundary |
| C headers | Fully retained, unchanged | Zero changes to C consumers |
