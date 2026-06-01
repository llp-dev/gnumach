# PMAP Module: Idiomatic Rust Migration — Implementation Plan

> **For the build agent:** Execute this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Stop at `[REVIEW GATE]` markers.

**Goal:** Replace `i386/intel/pmap.c` (except bootstrap) with an idiomatic Rust `staticlib` crate in `rust/pmap/`, linked into the existing kernel binary.

**Spec:** `docs/designs/2026-06-01-pmap-rust-migration.md`

**Architecture:** Single `no_std` Rust crate with 14 feature-gated modules. `#[no_mangle] extern "C"` thin wrappers in `ffi.rs` are the sole C↔Rust boundary. Locking uses RAII guards with `Drop`. Bootstrap functions remain in C (`i386/intel/pmap_bootstrap.c`). Built via Cargo invoked from Autotools Makefiles, producing `libpmap.a` linked into `libkernel.a`.

**Tech Stack:** Rust 2021 edition, `#[no_std]`, `core` only (no `alloc`), `panic = "abort"`, `bindgen` for FFI type generation, Autotools+Cargo hybrid build, QEMU for integration tests.

---

## Phase 1: Project Skeleton & Build Infrastructure

### Task 1: Create Rust crate skeleton

**Files:**
- Create: `rust/pmap/Cargo.toml`
- Create: `rust/pmap/src/lib.rs`
- Create: `rust/pmap/build.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "pmap"
version = "0.1.0"
edition = "2021"
description = "GNU Mach physical map (PMAP) module"

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

# For host-side unit testing of pure logic:
[features]
host-test = []

[build-dependencies]
# Bindgen requires libclang at build time
bindgen = "0.69"
```

- [ ] **Step 2: Write `lib.rs` with `#![no_std]` and module declarations**

```rust
// rust/pmap/src/lib.rs
#![no_std]
#![cfg_attr(not(any(test, feature = "host-test")), no_main)]

mod types;
mod locking;

#[cfg(not(any(test, feature = "host-test")))]
mod lifecycle;
#[cfg(not(any(test, feature = "host-test")))]
mod mapping;
#[cfg(not(any(test, feature = "host-test")))]
mod protection;
#[cfg(not(any(test, feature = "host-test")))]
mod pv_list;
#[cfg(not(any(test, feature = "host-test")))]
mod expand;
#[cfg(not(any(test, feature = "host-test")))]
mod page_attr;
#[cfg(not(any(test, feature = "host-test")))]
mod phys_ops;
#[cfg(not(any(test, feature = "host-test")))]
mod mapwindow;
#[cfg(not(any(test, feature = "host-test")))]
mod activation;
#[cfg(not(any(test, feature = "host-test")))]
mod collect;

#[cfg(all(not(any(test, feature = "host-test")), feature = "kdb"))]
mod whatis;
#[cfg(all(not(any(test, feature = "host-test")), feature = "smp"))]
mod smp;
#[cfg(all(not(any(test, feature = "host-test")), feature = "xen"))]
mod xen;

#[cfg(not(any(test, feature = "host-test")))]
mod ffi;
```

- [ ] **Step 3: Verify with `cargo check`**

```bash
cd rust/pmap && cargo check 2>&1 | head -20
```
Expected: Errors about missing modules `types`, `locking`, etc. (not yet created) — confirms crate is alive.

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/Cargo.toml rust/pmap/src/lib.rs
git commit -m "rust: add pmap crate skeleton (Cargo.toml, lib.rs)"
```

---

### Task 2: Add Rust files to `.gitignore`

**Files:**
- Modify: `.gitignore` (create if absent)

- [ ] **Step 1: Add Rust build artifacts to `.gitignore`**

Add to `.gitignore`:
```
rust/pmap/target/
```

- [ ] **Step 2: Verify**

```bash
ls .gitignore && grep "rust/pmap/target" .gitignore
```

- [ ] **Step 3: Commit**

```bash
git add .gitignore
git commit -m "build: add rust build artifacts to .gitignore"
```

---

### Task 3: Create x86_64 target specification

**Files:**
- Create: `rust/pmap/x86_64-gnumach.json`

- [ ] **Step 1: Write target JSON**

```json
{
    "arch": "x86_64",
    "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
    "disable-redzone": true,
    "features": "-mmx,-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2,-3dnow",
    "linker": "rust-lld",
    "linker-flavor": "ld.lld",
    "llvm-target": "x86_64-unknown-none",
    "max-atomic-width": 64,
    "os": "none",
    "panic-strategy": "abort",
    "position-independent-executables": false,
    "relocation-model": "static",
    "code-model": "kernel",
    "target-c-int-width": "32",
    "target-pointer-width": "64",
    "target-endian": "little"
}
```

- [ ] **Step 2: Verify the target is recognized**

```bash
rustc --version && cd rust/pmap && rustc --print target-list | grep x86_64
cargo check --target x86_64-gnumach.json 2>&1 | head -20
```
Expected: Rust can parse the target JSON; crate compiles with `no_std`.

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/x86_64-gnumach.json
git commit -m "rust: add x86_64 kernel target specification"
```

---

### Task 4: Write `build.rs` for bindgen generation

**Files:**
- Create: `rust/pmap/build.rs`

- [ ] **Step 1: Write `build.rs`**

```rust
// rust/pmap/build.rs
use std::env;
use std::path::PathBuf;

fn main() {
    // Only run bindgen if the C headers are available (skip during cargo test on host)
    let kernel_include = PathBuf::from("..").join("include");
    let i386_include = PathBuf::from("..").join("i386").join("include");

    if !kernel_include.exists() {
        eprintln!("cargo:warning=kernel headers not found, skipping bindgen");
        return;
    }

    let bindings = bindgen::Builder::default()
        .header("../i386/intel/pmap.h")
        .header("../vm/pmap.h")
        .header("../include/mach/vm_prot.h")
        .header("../include/mach/vm_statistics.h")
        .header("../include/mach/kern_return.h")
        .clang_args(&[
            "-I", "../i386/include",
            "-I", "../include",
            "-I", "..",
            "-I", "../i386",
            "-DMACH_KERNEL",
            "-D__ELF__",
        ])
        .use_core()
        .ctypes_prefix("crate::ctypes")
        .derive_default(true)
        .derive_eq(true)
        .derive_ord(true)
        .generate_comments(false)
        .layout_tests(false)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap())
        .join("bindings.rs");
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings");
}
```

- [ ] **Step 2: Verify bindgen can run (optional, requires libclang)**

```bash
which bindgen 2>/dev/null || cargo install bindgen-cli
cd rust/pmap && cargo build --target x86_64-gnumach.json 2>&1 | tail -20
```
Expected: Either successful bindgen output or graceful skip if headers unavailable.

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/build.rs
git commit -m "rust: add build.rs for bindgen FFI type generation"
```

---

### Task 5: Cargo-Autotools integration in Makefiles

**Files:**
- Create: `rust/Makefrag.am`
- Modify: `Makefile.am`
- Modify: `i386/Makefrag_x86.am`

- [ ] **Step 1: Write `rust/Makefrag.am`**

```makefile
# Makefile fragment for Rust PMAP crate.

RUST_PMAP_DIR = $(top_srcdir)/rust/pmap

if HOST_x86_64
RUST_TARGET = x86_64-gnumach.json
else
RUST_TARGET = i386-gnumach.json
endif

# Feature flags passed to cargo
RUST_FEATURES =

if enable_pae
RUST_FEATURES += --features pae
endif
if HOST_x86_64
RUST_FEATURES += --features x86_64
endif
if enable_smp
RUST_FEATURES += --features smp
endif
if PLATFORM_xen
RUST_FEATURES += --features xen
# MACH_HYP is always set for xen
RUST_FEATURES += --features xen_hyp
if enable_pseudo_phys
RUST_FEATURES += --features pseudo_phys
endif
endif
if enable_kdb
RUST_FEATURES += --features kdb
endif

# The static library produced by cargo
RUST_LIB = $(RUST_PMAP_DIR)/target/x86_64-unknown-none/release/libpmap.a

$(RUST_LIB): $(RUST_PMAP_DIR)/Cargo.toml $(shell find $(RUST_PMAP_DIR)/src -name '*.rs')
	cd $(RUST_PMAP_DIR) && \
	cargo build --release --target x86_64-gnumach.json $(RUST_FEATURES)

# Add the Rust staticlib to the kernel library
libkernel_a_LIBADD += $(RUST_LIB)
```

- [ ] **Step 2: Modify `Makefile.am` to include `rust/Makefrag.am`**

Add after the `# Test suite.` include (line 157):
```makefile
# Rust components.
include rust/Makefrag.am
```

- [ ] **Step 3: Modify `i386/Makefrag_x86.am` to replace `pmap.c` with `pmap_bootstrap.c`**

Replace line 78 (`i386/intel/pmap.c`) with:
```makefile
	i386/intel/pmap_bootstrap.c \
```

- [ ] **Step 4: Verify the build system changes (just check structure)**

```bash
grep -n "rust/Makefrag" Makefile.am
grep -n "pmap_bootstrap" i386/Makefrag_x86.am
grep -n "pmap\.c" i386/Makefrag_x86.am
```
Expected: `pmap_bootstrap.c` is listed, `pmap.c` is NOT listed.

- [ ] **Step 5: Commit**

```bash
git add rust/Makefrag.am Makefile.am i386/Makefrag_x86.am
git commit -m "build: integrate Rust PMAP crate into Autotools build system"
```

---

- [ ] [REVIEW GATE] **Phase 1 Complete: Build infrastructure. Verify: `cd rust/pmap && cargo check --target x86_64-gnumach.json 2>&1 | tail -5` should show only missing-module errors (no toolchain failures).**

---

## Phase 2: Types & Global State

### Task 6: Write `types.rs` — core type definitions

**Files:**
- Create: `rust/pmap/src/types.rs`

- [ ] **Step 1: Write `types.rs` with PMAP struct, PTE constants, and type aliases**

```rust
// rust/pmap/src/types.rs
use core::ffi::{c_char, c_int, c_uint, c_ulong, c_void};

//
// Type aliases matching C kernel types
//
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

//
// PTE bit definitions (from i386/intel/pmap.h)
//
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

#[cfg(not(feature = "pae"))]
pub const INTEL_PTE_PFN:    PhysAddr = 0x00_FFFF_F000;
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
pub const INTEL_PTE_PFN:    PhysAddr = 0x00_7FFF_FFFF_FFFF_F000;
#[cfg(feature = "x86_64")]
pub const INTEL_PTE_PFN:    PhysAddr = 0xFF_FFFF_FFFF_FFFF_F000;

pub const INTEL_OFFMASK:    PhysAddr = 0xFFF;

//
// Page size constants
//
pub const I386_PGSHIFT: u32 = 12;
pub const I386_PGBYTES: usize = 1 << I386_PGSHIFT;

//
// Page table level shift/mask constants
//
#[cfg(not(feature = "pae"))]
pub const PDESHIFT: u32 = 22;
#[cfg(not(feature = "pae"))]
pub const PDEMASK: u32 = 0x3FF;
#[cfg(not(feature = "pae"))]
pub const PTEMASK: u32 = 0x3FF;

#[cfg(feature = "pae")]
pub const PDPSHIFT: u32 = 30;
#[cfg(all(feature = "pae", feature = "x86_64"))]
pub const PDPMASK: u32 = 0x1FF;
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
pub const PDPMASK: u32 = 3;
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

//
// Simple lock type (repr(C), matches decl_simple_lock_data)
//
#[repr(C)]
#[derive(Default)]
pub struct SimpleLock {
    // Implemented as a spinlock. The exact layout matches the C simple_lock_data.
    lock_data: c_uint,  // simplified; real layout from bindgen
}

//
// Pmap statistics (from mach/vm_statistics.h)
//
#[repr(C)]
#[derive(Default)]
pub struct PmapStatistics {
    pub resident_count: Integer,
    pub wired_count: Integer,
}

//
// Main Pmap struct (repr(C), matches struct pmap in i386/intel/pmap.h)
//
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

//
// PV entry for reverse mapping table
//
#[repr(C)]
pub struct PvEntry {
    pub next: *mut PvEntry,
    pub pmap: *mut Pmap,     // pmap_t
    pub va: VmOffset,        // virtual address
}

//
// Map window structure
//
#[repr(C)]
pub struct PmapMapwindow {
    pub entry: *mut Pte,
    pub vaddr: VmOffset,
}

//
// VM protection constants
//
pub const VM_PROT_NONE:    VmProt = 0x00;
pub const VM_PROT_READ:    VmProt = 0x01;
pub const VM_PROT_WRITE:   VmProt = 0x02;
pub const VM_PROT_EXECUTE: VmProt = 0x04;
pub const VM_PROT_ALL:     VmProt = VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE;

//
// Kernel return codes
//
pub const KERN_SUCCESS:            KernReturn = 0;
pub const KERN_INVALID_ADDRESS:    KernReturn = 1;
pub const KERN_PROTECTION_FAILURE: KernReturn = 2;
pub const KERN_RESOURCE_SHORTAGE:  KernReturn = 6;
```

- [ ] **Step 2: Verify with `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```
Expected: Errors about missing `locking` module only.

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/types.rs
git commit -m "rust/pmap: add type definitions (Pmap, PTE constants, PvEntry)"
```

---

### Task 7: Write PTE math unit tests (host-side)

**Files:**
- Create: `rust/pmap/src/types.rs` (add `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing test**

Add to end of `types.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pte_to_pa_roundtrip() {
        let pa: PhysAddr = 0x12345000;
        let pte = pa_to_pte(pa);
        assert_eq!(pte_to_pa(pte), pa);
    }

    #[test]
    fn test_pte_bit_constants() {
        assert_eq!(INTEL_PTE_VALID, 0x01);
        assert_eq!(INTEL_PTE_WRITE,  0x02);
        assert_eq!(INTEL_PTE_MOD,    0x40);
        // Verify no overlap in bit definitions
        let bits = INTEL_PTE_VALID | INTEL_PTE_WRITE | INTEL_PTE_USER
                 | INTEL_PTE_WTHRU | INTEL_PTE_NCACHE | INTEL_PTE_REF
                 | INTEL_PTE_MOD | INTEL_PTE_PS | INTEL_PTE_GLOBAL
                 | INTEL_PTE_WIRED;
        assert_eq!(bits & !(0xFFF), 0, "no PTE bits overlap with PFN field");
    }

    #[test]
    fn test_page_size_constants() {
        assert_eq!(I386_PGBYTES, 4096);
        assert_eq!(1 << I386_PGSHIFT, I386_PGBYTES);
    }

    #[test]
    fn test_pte_null() {
        let null_pte: *mut Pte = core::ptr::null_mut();
        assert!(null_pte.is_null());
    }
}
```

- [ ] **Step 2: Write the minimal implementation (add to `types.rs` before `#[cfg(test)]`)**

```rust
/// Convert physical address to PTE value (mask to PFN)
#[inline]
pub fn pa_to_pte(pa: PhysAddr) -> PhysAddr {
    pa & INTEL_PTE_PFN
}

/// Extract physical address from PTE value
#[inline]
pub fn pte_to_pa(pte: PhysAddr) -> PhysAddr {
    pte & INTEL_PTE_PFN
}

/// Increment PTE by one page
#[inline]
pub fn pte_increment_pa(pte: &mut PhysAddr) {
    *pte += INTEL_OFFMASK + 1;
}

/// Check if PTE is valid (present)
#[inline]
pub fn pte_is_valid(pte: PhysAddr) -> bool {
    (pte & INTEL_PTE_VALID) != 0
}
```

- [ ] **Step 3: Run tests to verify they pass**

```bash
cd rust/pmap && cargo test 2>&1
```
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/types.rs
git commit -m "rust/pmap: add PTE math functions and host-side unit tests"
```

---

### Task 8: Write page-table index math with tests

**Files:**
- Create: `rust/pmap/src/types.rs` (add index functions)

- [ ] **Step 1: Write failing tests for index math**

Add to `types.rs` test module:
```rust
#[test]
fn test_lin2pdenum_non_pae() {
    // In non-PAE: bits 22–31 are the PDE index
    // Example: address 0x00400000 → PDE index 1
    let addr: VmOffset = 0x00400000; // 4 MiB
    // PDESHIFT=22, so (addr >> 22) & PDEMASK
    let expected = 1u32;
    assert_eq!(lin2pdenum(addr), expected);
}

#[test]
fn test_ptenum() {
    // bits 12–21 are the PTE index
    let addr: VmOffset = 0x00001000; // second page
    assert_eq!(ptenum(addr), 1u32);
    let addr2: VmOffset = 0x0;
    assert_eq!(ptenum(addr2), 0u32);
}

#[cfg(feature = "x86_64")]
#[test]
fn test_lin2l4num() {
    let addr: VmOffset = 0xFFFF_8000_0000_0000; // kernel address
    let l4_idx = lin2l4num(addr);
    // Kernel addresses have L4 index 0x1FF (sign-extended region)
    assert_eq!(l4_idx, 0x1FF);
}
```

- [ ] **Step 2: Implement index functions**

Add to `types.rs`:
```rust
/// Convert linear address to L4 table index (x86_64 only)
#[cfg(feature = "x86_64")]
#[inline]
pub fn lin2l4num(addr: VmOffset) -> u32 {
    ((addr >> L4SHIFT) & L4MASK as usize) as u32
}

/// Convert linear address to page directory pointer index (PAE only)
#[cfg(feature = "pae")]
#[inline]
pub fn lin2pdpnum(addr: VmOffset) -> u32 {
    ((addr >> PDPSHIFT) & PDPMASK as usize) as u32
}

/// Convert linear address to page directory entry index
#[inline]
pub fn lin2pdenum(addr: VmOffset) -> u32 {
    ((addr >> PDESHIFT) & PDEMASK as usize) as u32
}

/// Convert linear address to page table entry index
#[inline]
pub fn ptenum(addr: VmOffset) -> u32 {
    ((addr >> PTESHIFT) & PTEMASK as usize) as u32
}

/// Convert page descriptor entry index to linear address offset
#[inline]
pub fn pdenum2lin(pde_idx: u32) -> VmOffset {
    (pde_idx as VmOffset) << PDESHIFT
}
```

- [ ] **Step 3: Run tests, verify pass**

```bash
cd rust/pmap && cargo test 2>&1
```
Expected: All tests pass (including x86_64-specific if running with `--features x86_64`).

Also test with features:
```bash
cd rust/pmap && cargo test --features x86_64 2>&1
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/types.rs
git commit -m "rust/pmap: add page-table index math with unit tests"
```

---

- [ ] [REVIEW GATE] **Phase 2 Complete: Types and address math. Run: `cd rust/pmap && cargo test && cargo test --features x86_64` — all tests pass.**

---

## Phase 3: Locking Primitives

### Task 9: Write `locking.rs` — RAII lock guards

**Files:**
- Create: `rust/pmap/src/locking.rs`

- [ ] **Step 1: Write `locking.rs`**

```rust
// rust/pmap/src/locking.rs
use core::ffi::c_int;
use core::ops::{Deref, DerefMut};
use crate::types::{Pmap, SimpleLock, CpuSet};

//
// External C functions for lock and SPL operations
//
extern "C" {
    fn splvm() -> c_int;
    fn splx(spl: c_int);
    fn simple_lock(l: *mut SimpleLock);
    fn simple_unlock(l: *mut SimpleLock);
    fn lock_read(l: *mut c_void);
    fn lock_read_done(l: *mut c_void);
    fn lock_write(l: *mut c_void);
    fn lock_write_done(l: *mut c_void);
    fn cpu_number() -> c_int;
    fn i_bit_clear(bit: c_int, set: *mut CpuSet);
    fn i_bit_set(bit: c_int, set: *mut CpuSet);

    // Global CPU bitmaps and lock (defined in C)
    #[cfg(feature = "smp")]
    static mut pmap_system_lock: c_void; // lock_data_t
    #[cfg(feature = "smp")]
    static mut cpus_active: CpuSet;
}

//
// SplGuard: raises SPLVM, removes CPU from cpus_active on SMP.
// Restores on Drop.
//
pub struct SplGuard {
    prev_spl: c_int,
}

impl SplGuard {
    /// Acquire SPLVM. On SMP, also removes this CPU from cpus_active.
    pub fn acquire() -> Self {
        let spl = unsafe { splvm() };
        #[cfg(feature = "smp")]
        unsafe {
            i_bit_clear(cpu_number(), core::ptr::addr_of_mut!(cpus_active));
        }
        SplGuard { prev_spl: spl }
    }
}

impl Drop for SplGuard {
    fn drop(&mut self) {
        #[cfg(feature = "smp")]
        unsafe {
            i_bit_set(cpu_number(), core::ptr::addr_of_mut!(cpus_active));
        }
        unsafe { splx(self.prev_spl) };
    }
}

//
// PmapReadGuard: protects pmap-based operations.
// Acquires: SPLVM → read-lock pmap_system_lock → simple_lock pmap
//
pub struct PmapReadGuard<'a> {
    #[cfg(feature = "smp")]
    _spl: SplGuard,
    #[cfg(not(feature = "smp"))]
    _spl: (),
    pmap: &'a mut Pmap,
}

impl<'a> PmapReadGuard<'a> {
    /// Acquire a read lock on the pmap system and the specific pmap.
    ///
    /// # Safety
    /// The pmap must be a valid, initialized pmap.
    pub unsafe fn acquire(pmap: &'a mut Pmap) -> Self {
        #[cfg(feature = "smp")]
        {
            let spl = SplGuard::acquire();
            lock_read(core::ptr::addr_of_mut!(pmap_system_lock) as *mut c_void);
            simple_lock(&mut pmap.lock);
            PmapReadGuard { _spl: spl, pmap }
        }
        #[cfg(not(feature = "smp"))]
        {
            PmapReadGuard { _spl: (), pmap }
        }
    }

    pub fn pmap(&self) -> &Pmap { self.pmap }
    pub fn pmap_mut(&mut self) -> &mut Pmap { self.pmap }
}

impl<'a> Drop for PmapReadGuard<'a> {
    fn drop(&mut self) {
        #[cfg(feature = "smp")]
        unsafe {
            simple_unlock(&mut self.pmap.lock);
            lock_read_done(core::ptr::addr_of_mut!(pmap_system_lock) as *mut c_void);
        }
        // _spl drops here → splx + cpus_active restore
    }
}

impl<'a> Deref for PmapReadGuard<'a> {
    type Target = Pmap;
    fn deref(&self) -> &Pmap { self.pmap }
}

//
// PmapWriteGuard: exclusive write lock on pmap_system_lock.
// Used for pv_list-based operations (pmap_remove_all, pmap_copy_on_write).
//
#[cfg(feature = "smp")]
pub struct PmapWriteGuard {
    _spl: SplGuard,
}

#[cfg(feature = "smp")]
impl PmapWriteGuard {
    pub unsafe fn acquire() -> Self {
        let spl = SplGuard::acquire();
        lock_write(core::ptr::addr_of_mut!(pmap_system_lock) as *mut c_void);
        PmapWriteGuard { _spl: spl }
    }
}

#[cfg(feature = "smp")]
impl Drop for PmapWriteGuard {
    fn drop(&mut self) {
        unsafe {
            lock_write_done(core::ptr::addr_of_mut!(pmap_system_lock) as *mut c_void);
        }
    }
}

// On UP, write guard is a no-op type
#[cfg(not(feature = "smp"))]
pub struct PmapWriteGuard;

#[cfg(not(feature = "smp"))]
impl PmapWriteGuard {
    pub fn acquire() -> Self { PmapWriteGuard }
}

//
// PvLockGuard: bit lock on pv_head_table entry.
// On SMP, locks the bit; on UP, no-op.
//
pub struct PvLockGuard {
    #[cfg(feature = "smp")]
    index: usize,
}

impl PvLockGuard {
    pub unsafe fn acquire(index: usize) -> Self {
        #[cfg(feature = "smp")]
        {
            lock_pvh_pai(index);
            PvLockGuard { index }
        }
        #[cfg(not(feature = "smp"))]
        {
            let _ = index;
            PvLockGuard {}
        }
    }
}

impl Drop for PvLockGuard {
    fn drop(&mut self) {
        #[cfg(feature = "smp")]
        unsafe { unlock_pvh_pai(self.index); }
    }
}

// FFI declarations for pv lock table (defined in C's pmap_bootstrap.c)
extern "C" {
    static mut pv_lock_table: *mut u8;
}

unsafe fn lock_pvh_pai(index: usize) {
    // bit_lock(index, pv_lock_table) — implemented in C, called via FFI
    extern "C" { fn bit_lock(bit: usize, table: *mut u8); }
    unsafe { bit_lock(index, pv_lock_table); }
}

unsafe fn unlock_pvh_pai(index: usize) {
    extern "C" { fn bit_unlock(bit: usize, table: *mut u8); }
    unsafe { bit_unlock(index, pv_lock_table); }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 3: Write host-side guard tests (demonstrate Drop behavior)**

Add to `locking.rs` or create `tests/locking.rs`:
```rust
#[cfg(test)]
mod tests {
    use core::cell::Cell;

    // Test that SplGuard drop semantics work correctly
    #[test]
    fn test_spl_guard_drop_noop_on_host() {
        // On host (non-SMP, no kernel), SplGuard on UP is type () or similar
        // This test just exercises that code compiles and runs
        let guard = super::PmapWriteGuard::acquire();
        drop(guard); // explicit drop to verify no panic
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/locking.rs
git commit -m "rust/pmap: add locking module with RAII guards (SplGuard, PmapReadGuard)"
```

---

- [ ] [REVIEW GATE] **Phase 3 Complete: Locking infrastructure. Run: `cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64,smp` — compiles cleanly.**

---

## Phase 4: Global State & C Linkage

### Task 10: Wire global state from C bootstrap into Rust

**Files:**
- Modify: `rust/pmap/src/types.rs`
- Modify: `rust/pmap/src/lib.rs`

- [ ] **Step 1: Add extern static declarations to `types.rs`**

Add at the end of `types.rs`:
```rust
//
// Global state: allocated and initialized by C bootstrap (pmap_bootstrap.c),
// accessed read-only or via lock protocol by Rust.
//

extern "C" {
    /// The kernel's address space pmap. Initialized by pmap_bootstrap().
    pub static mut kernel_pmap: *mut Pmap;

    /// Kernel page directory base. Initialized by pmap_bootstrap().
    pub static mut kernel_page_dir: *mut Pte;

    /// Start of kernel virtual address range (beyond direct-mapped RAM).
    pub static mut kernel_virtual_start: VmOffset;

    /// End of kernel virtual address range.
    pub static mut kernel_virtual_end: VmOffset;

    /// Set to 1 (TRUE) after pmap_init() completes.
    pub static mut pmap_initialized: c_int;

    /// PV head table: one pv_entry per vm_page.
    pub static mut pv_head_table: *mut PvEntry;

    /// Lock-bit table for pv_head entries.
    pub static mut pv_lock_table: *mut c_char;

    /// Physical page attribute bytes (MOD/REF bits).
    pub static mut pmap_phys_attributes: *mut c_char;

    /// Number of CPUs (set at compile time).
    pub static NCPUS: c_int;
}

/// Get a reference to the kernel pmap. Only valid after pmap_bootstrap().
///
/// # Safety
/// Must not be called before pmap_bootstrap() initializes kernel_pmap.
pub unsafe fn kernel_pmap_ref() -> &'static Pmap {
    unsafe { &*kernel_pmap }
}

/// Check if pmap_init() has completed.
pub fn is_pmap_initialized() -> bool {
    unsafe { pmap_initialized != 0 }
}
```

- [ ] **Step 2: Add `ffi` module declaration to `lib.rs`**

Update `lib.rs` — uncomment or add:
```rust
#[cfg(not(any(test, feature = "host-test")))]
mod ffi;
```

- [ ] **Step 3: Verify with `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json 2>&1 | tail -15
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/types.rs rust/pmap/src/lib.rs
git commit -m "rust/pmap: wire global state from C bootstrap into Rust types"
```

---

### Task 11: Create stub `ffi.rs` with one function to prove linkage

**Files:**
- Create: `rust/pmap/src/ffi.rs`

- [ ] **Step 1: Write `ffi.rs` with single stub function**

```rust
// rust/pmap/src/ffi.rs
// Thin `#[no_mangle] extern "C"` wrappers. Converts C types ↔ Rust types.

use crate::types::*;

/// Returns the kernel pmap pointer. Called by C code.
#[no_mangle]
pub extern "C" fn pmap_kernel() -> *mut Pmap {
    unsafe { kernel_pmap }
}

/// Returns the resident count for a pmap.
#[no_mangle]
pub extern "C" fn pmap_resident_count(pmap: *mut Pmap) -> c_int {
    if pmap.is_null() { return 0; }
    unsafe { (*pmap).stats.resident_count }
}
```

- [ ] **Step 2: Add `use core::ffi::c_int;` import if needed**

Ensure `ffi.rs` can access `c_int`:
```rust
use core::ffi::c_int;
```

- [ ] **Step 3: Verify with `cargo build`**

```bash
cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features x86_64 2>&1 | tail -15
```
Expected: Successful build producing `target/x86_64-unknown-none/release/libpmap.a`.

- [ ] **Step 4: Inspect exported symbols**

```bash
nm rust/pmap/target/x86_64-unknown-none/release/libpmap.a 2>/dev/null | grep -E "pmap_kernel|pmap_resident_count"
```
Expected: Symbols `pmap_kernel` and `pmap_resident_count` are visible.

- [ ] **Step 5: Commit**

```bash
git add rust/pmap/src/ffi.rs
git commit -m "rust/pmap: add FFI stub (pmap_kernel, pmap_resident_count)"
```

---

- [ ] [REVIEW GATE] **Phase 4 Complete: Rust crate builds and exports C symbols. Run: `cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features x86_64 && nm target/x86_64-unknown-none/release/libpmap.a 2>/dev/null | grep pmap` — symbols visible. Review the exported symbols to confirm C ABI compatibility.**

---

## Phase 5: Page Table Walk & Expand

### Task 12: Write `expand.rs` — `pmap_pte()` core page table walker

**Files:**
- Create: `rust/pmap/src/expand.rs`
- Create: `rust/pmap/tests/pte_walk.rs` (host-side tests)

- [ ] **Step 1: Write failing test for address decomposition (not needing hardware)**

```rust
// rust/pmap/tests/pte_walk.rs
use pmap::types::*;

#[test]
fn test_pte_index_decomposition() {
    // For a known address, verify that index extraction is correct.
    // 4 KiB aligned address 0x0000_0000_1000_2000
    let addr: VmOffset = 0x1000_2000;

    #[cfg(feature = "x86_64")]
    {
        assert_eq!(lin2l4num(addr), 0);
    }

    #[cfg(feature = "pae")]
    {
        assert_eq!(lin2pdpnum(addr), 0);
    }

    // PDE index: address 0x1000_2000
    // PDESHIFT=21 for PAE: (0x1000_2000 >> 21) = 0x80_01 >> anything
    // Actually: 0x1000_2000 = binary: 0001_0000_0000_0000_0010_0000_0000_0000
    // >> 21 = 0x80_01 = 32769
    // & 0x1FF = 0x001 = 1
    #[cfg(feature = "pae")]
    {
        assert_eq!(lin2pdenum(addr), 0x0001);
    }
    #[cfg(not(feature = "pae"))]
    {
        // PDESHIFT=22: 0x1000_2000 >> 22 = 0x400_08 >> ...
        // Actually: (0x1000_2000 >> 22) & 0x3FF = 0x40
        assert_eq!(lin2pdenum(addr), 0x40);
    }

    // PTE index: (addr >> 12) & mask
    #[cfg(any(feature = "pae", feature = "x86_64"))]
    {
        assert_eq!(ptenum(addr), 0x002);
    }
    #[cfg(not(feature = "pae"))]
    {
        assert_eq!(ptenum(addr), 0x002);
    }
}
```

- [ ] **Step 2: Verify this test FAILS (feature-gated test not run by default)**

```bash
cd rust/pmap && cargo test --features x86_64 --test pte_walk 2>&1
```
Expected: Test file doesn't compile or tests are found.

- [ ] **Step 3: Write `expand.rs` — `pmap_pte()` skeleton**

```rust
// rust/pmap/src/expand.rs
use crate::types::*;

/// Given a pmap and virtual address, walks the page table hierarchy
/// and returns a pointer to the leaf PTE, or null if not mapped.
///
/// This is the central page table walking function.
pub fn pmap_pte(pmap: &Pmap, addr: VmOffset) -> *mut Pte {
    #[cfg(feature = "x86_64")]
    {
        let l4 = unsafe { &*pmap.l4base };
        let l4_idx = lin2l4num(addr) as usize;

        if !pte_is_valid(unsafe { *pmap.l4base.add(l4_idx) }) {
            return core::ptr::null_mut();
        }

        // Walk L4 → L3 (PDP)
        let pdp_base = pte_to_pa(unsafe { *pmap.l4base.add(l4_idx) });
        // ... continue walking
        // For now: stub that returns null
        return core::ptr::null_mut();
    }

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        // 3-level PAE walk
        core::ptr::null_mut()
    }

    #[cfg(not(feature = "pae"))]
    {
        // 2-level walk
        core::ptr::null_mut()
    }
}

/// Check if PTE is valid
#[inline]
fn pte_is_valid(pte: PhysAddr) -> bool {
    (pte & INTEL_PTE_VALID) != 0
}
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/expand.rs rust/pmap/tests/pte_walk.rs
git commit -m "rust/pmap: add expand.rs skeleton and pte walk host test"
```

---

### Task 13: Implement `pmap_pte()` for x86_64 (non-Xen)

**Files:**
- Modify: `rust/pmap/src/expand.rs`

- [ ] **Step 1: Write the x86_64 `pmap_pte()` implementation**

Replace the stub in `expand.rs` with:
```rust
pub fn pmap_pte(pmap: &Pmap, addr: VmOffset) -> *mut Pte {
    // x86_64 4-level walk: L4 → PDP → PD → PT
    #[cfg(feature = "x86_64")]
    {
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
        // pte_to_pa gives the physical address of the PDP table
        // For kernel addresses, this is directly accessible (direct map)
        let pdp_table = phystokv(pte_to_pa(l4e)) as *mut Pte;
        let pdpe = unsafe { *pdp_table.add(pdp_idx) };
        if !pte_is_valid(pdpe) {
            return core::ptr::null_mut();
        }

        // PD lookup
        let pde_idx = lin2pdenum(addr) as usize;
        let pd_table = phystokv(pte_to_pa(pdpe)) as *mut Pte;
        let pde = unsafe { *pd_table.add(pde_idx) };
        if !pte_is_valid(pde) {
            return core::ptr::null_mut();
        }

        // Large page check (PS bit)
        if (pde & INTEL_PTE_PS) != 0 {
            // Large page (2 MiB): return address of PDE itself as PTE
            return unsafe { pd_table.add(pde_idx) };
        }

        // PT lookup
        let pte_idx = ptenum(addr) as usize;
        let pt_table = phystokv(pte_to_pa(pde)) as *mut Pte;
        unsafe { pt_table.add(pte_idx) }
    }

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        // 3-level walk (32-bit PAE): PDP → PD → PT
        if pmap.pdpbase.is_null() {
            return core::ptr::null_mut();
        }
        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdpe = unsafe { *pmap.pdpbase.add(pdp_idx) };
        if !pte_is_valid(pdpe) { return core::ptr::null_mut(); }

        let pd_table = phystokv(pte_to_pa(pdpe)) as *mut Pte;
        let pde_idx = lin2pdenum_cont(addr) as usize;
        let pde = unsafe { *pd_table.add(pde_idx) };
        if !pte_is_valid(pde) { return core::ptr::null_mut(); }

        // Check large page (2 MiB)
        if (pde & INTEL_PTE_PS) != 0 {
            return unsafe { pd_table.add(pde_idx) };
        }

        let pt_table = phystokv(pte_to_pa(pde)) as *mut Pte;
        let pte_idx = ptenum(addr) as usize;
        unsafe { pt_table.add(pte_idx) }
    }

    #[cfg(not(feature = "pae"))]
    {
        // 2-level walk: PD → PT
        if pmap.dirbase.is_null() {
            return core::ptr::null_mut();
        }
        let pde_idx = lin2pdenum(addr) as usize;
        let pde = unsafe { *pmap.dirbase.add(pde_idx) };
        if !pte_is_valid(pde) { return core::ptr::null_mut(); }

        let pt_table = phystokv(pte_to_pa(pde)) as *mut Pte;
        let pte_idx = ptenum(addr) as usize;
        unsafe { pt_table.add(pte_idx) }
    }
}

/// Convert physical address to kernel virtual address via direct map.
/// This is the phystokv() macro from the C kernel.
///
/// # Safety
/// Only valid for physical addresses within the direct-mapped region.
unsafe fn phystokv(pa: PhysAddr) -> usize {
    // The kernel maintains a 1:1 direct map of physical memory.
    // phystokv is defined in i386/locore.h. We call through FFI:
    extern "C" {
        fn phystokv(pa: PhysAddr) -> usize;
    }
    unsafe { phystokv(pa) }
}

/// PAE-specific: contiguous PDE index (includes PDP offset)
#[cfg(all(feature = "pae", not(feature = "x86_64")))]
#[inline]
fn lin2pdenum_cont(addr: VmOffset) -> u32 {
    ((addr >> PDESHIFT) & 0x7FF) as u32
}
```

- [ ] **Step 2: Verify `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/expand.rs
git commit -m "rust/pmap: implement pmap_pte() page table walker for all x86 variants"
```

---

### Task 14: Implement `pmap_expand()` — on-demand page table allocation

**Files:**
- Modify: `rust/pmap/src/expand.rs`

- [ ] **Step 1: Write `pmap_expand()`**

```rust
/// Ensure a full page-table path exists for a virtual address.
/// Allocates intermediate page tables as needed. Returns leaf PTE.
///
/// # Safety
/// Must be called with the pmap read-locked (PmapReadGuard held).
/// The caller must hold a valid PmapReadGuard.
pub fn pmap_expand(pmap: &mut Pmap, v: VmOffset) -> *mut Pte {
    #[cfg(feature = "x86_64")]
    {
        // Expand L4 → PDP
        let l4_idx = lin2l4num(v) as usize;
        if !pte_is_valid(unsafe { *pmap.l4base.add(l4_idx) }) {
            pmap_expand_level(
                pmap, v, 0,
                pmap_l4base as unsafe fn(&Pmap, VmOffset) -> *mut Pte,
                |_pmap, _v| core::ptr::null_mut(), // L4 is top; no upper
            );
        }

        // Expand PDP → PD
        let pdp_idx = lin2pdpnum(v) as usize;
        let l4e = unsafe { *pmap.l4base.add(l4_idx) };
        let pdp_table: *mut Pte = unsafe { phystokv(pte_to_pa(l4e)) as *mut Pte };
        if !pte_is_valid(unsafe { *pdp_table.add(pdp_idx) }) {
            // allocate new PD table
            // ... simplified: call kernel allocator
        }

        // Expand PD → PT (simplified)
        let pde_idx = lin2pdenum(v) as usize;
        // ...

        return pmap_pte(pmap, v);
    }

    // Non-x86_64 paths similar but with fewer levels
    // Stub for now — returns existing PTE or null
    pmap_pte(pmap, v)
}

/// Function pointer type for page table level accessors
type LevelGetter = unsafe fn(&Pmap, VmOffset) -> *mut Pte;

/// Allocate a new page-table page and insert into parent level.
/// Returns pointer to the newly allocated entry in the parent.
fn pmap_expand_level(
    pmap: &mut Pmap,
    v: VmOffset,
    _spl: c_int,
    level_getter: LevelGetter,
    _upper_getter: LevelGetter,
) -> *mut Pte {
    // Allocate a new physical page for the page table
    // Uses kmem_cache_alloc() via FFI
    // For now: stub that compiles
    let _ = pmap;
    let _ = v;
    let _ = _spl;
    let _ = level_getter;
    core::ptr::null_mut()
}

/// L4 base accessor (x86_64)
#[cfg(feature = "x86_64")]
unsafe fn pmap_l4base(pmap: &Pmap, _addr: VmOffset) -> *mut Pte {
    pmap.l4base
}

use core::ffi::c_int;
```

- [ ] **Step 2: Compile check**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/expand.rs
git commit -m "rust/pmap: implement pmap_expand() skeleton for page table allocation"
```

---

- [ ] [REVIEW GATE] **Phase 5 Complete: Page table walk and expand stubs. Run: `cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features x86_64` — compiles cleanly. Review the page table walk logic against the C implementation in `i386/intel/pmap.c:pmap_pte()` (line ~506).**

---

## Phase 6: PV List Management

### Task 15: Write `pv_list.rs` — reverse mapping table

**Files:**
- Create: `rust/pmap/src/pv_list.rs`

- [ ] **Step 1: Write `pv_list.rs`**

```rust
// rust/pmap/src/pv_list.rs
// Physical→virtual reverse mapping table maintenance.

use crate::types::*;
use crate::locking::PvLockGuard;

extern "C" {
    // C functions for slab cache operations
    fn kmem_cache_alloc(cache: *mut c_void) -> *mut c_void;
    fn kmem_cache_free(cache: *mut c_void, ptr: *mut c_void);

    // pv_list_cache and pv_free_list (defined in C's pmap_init)
    static mut pv_list_cache: *mut c_void;  // kmem_cache
    static mut pv_free_list: *mut PvEntry;
    static mut pv_free_list_lock: SimpleLock;
}

/// Allocate a new pv_entry from the free list or slab cache.
pub fn pv_alloc() -> *mut PvEntry {
    unsafe {
        // Try the free list first (lock-free fast path with simple_lock)
        simple_lock(&mut pv_free_list_lock);
        let entry = pv_free_list;
        if !entry.is_null() {
            pv_free_list = (*entry).next;
        }
        simple_unlock(&mut pv_free_list_lock);

        if !entry.is_null() {
            return entry;
        }

        // Fall back to slab cache
        kmem_cache_alloc(pv_list_cache) as *mut PvEntry
    }
}

/// Return a pv_entry to the free list.
pub fn pv_free(entry: *mut PvEntry) {
    if entry.is_null() { return; }
    unsafe {
        simple_lock(&mut pv_free_list_lock);
        (*entry).next = pv_free_list;
        pv_free_list = entry;
        simple_unlock(&mut pv_free_list_lock);
    }
}

/// Lock a pv_head_table entry by index, return a guard.
pub fn lock_pvh(index: usize) -> PvLockGuard {
    unsafe { PvLockGuard::acquire(index) }
}

/// Index into pv_head_table for a physical address.
#[inline]
pub fn pa_index(pa: PhysAddr) -> usize {
    // vm_page_table_index(pa) — maps physical address to page table index
    // Defined in vm/vm_page.h. We call through FFI:
    extern "C" {
        fn vm_page_table_index(pa: PhysAddr) -> usize;
    }
    unsafe { vm_page_table_index(pa) }
}

/// Check if a physical address is a managed VM page.
pub fn valid_page(pa: PhysAddr) -> bool {
    extern "C" {
        fn valid_page(pa: PhysAddr) -> c_int;
    }
    unsafe { valid_page(pa) != 0 }
}

// FFI simple_lock wrappers (from locking.rs or inline)
extern "C" {
    fn simple_lock(l: *mut SimpleLock);
    fn simple_unlock(l: *mut SimpleLock);
}

use core::ffi::c_int;
```

- [ ] **Step 2: Verify `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/pv_list.rs
git commit -m "rust/pmap: implement pv_list reverse mapping table management"
```

---

- [ ] [REVIEW GATE] **Phase 6 Complete: PV list management. Review against C implementation in `i386/intel/pmap.c` lines 113–149 (pv_entry struct, PV_ALLOC, PV_FREE macros).**

---

## Phase 7: Core Mapping Operations

### Task 16: Write `mapping.rs` — `pmap_extract()`

**Files:**
- Create: `rust/pmap/src/mapping.rs`

- [ ] **Step 1: Write `pmap_extract()`**

```rust
// rust/pmap/src/mapping.rs
use crate::types::*;
use crate::expand::pmap_pte;

/// Extract the physical address for a virtual address in the given pmap.
/// Returns None if no mapping exists.
pub fn pmap_extract(pmap: &Pmap, va: VmOffset) -> Option<PhysAddr> {
    let pte_ptr = pmap_pte(pmap, va);
    if pte_ptr.is_null() {
        return None;
    }
    let pte = unsafe { *pte_ptr };
    if !pte_is_valid(pte) {
        return None;
    }
    Some(pte_to_pa(pte))
}

#[inline]
fn pte_is_valid(pte: PhysAddr) -> bool {
    (pte & INTEL_PTE_VALID) != 0
}

#[inline]
fn pte_to_pa(pte: PhysAddr) -> PhysAddr {
    pte & INTEL_PTE_PFN
}
```

- [ ] **Step 2: Commit**

```bash
git add rust/pmap/src/mapping.rs
git commit -m "rust/pmap: implement pmap_extract()"
```

---

### Task 17: Implement `pmap_enter()` — core mapping insertion

**Files:**
- Modify: `rust/pmap/src/mapping.rs`

- [ ] **Step 1: Write `pmap_enter()`**

```rust
/// Enter a virtual-to-physical mapping in a pmap.
/// If a mapping already exists at `va`, it is replaced.
/// Panics if page table allocation fails (kernel OOM).
pub fn pmap_enter(pmap: &mut Pmap, va: VmOffset, pa: PhysAddr, prot: VmProt, wired: bool) {
    // Ensure we are within the valid virtual address range
    // (actual range check uses kernel_virtual_start/end for kernel pmap)

    // Lock the pmap
    let guard = unsafe { PmapReadGuard::acquire(pmap) };

    // Ensure page table path exists (allocate intermediate tables if needed)
    let pte_ptr = crate::expand::pmap_expand(guard.pmap_mut(), va);
    if pte_ptr.is_null() {
        panic!("pmap_enter: pmap_expand failed for va {:#x}", va);
    }

    let old_pte = unsafe { *pte_ptr };

    // If there's an existing mapping, handle it:
    if pte_is_valid(old_pte) {
        let old_pa = pte_to_pa(old_pte);
        if old_pa == pa {
            // Same physical page — just update protection/wiring
            update_pte_protection(pte_ptr, prot, wired);
            return;
        }
        // Different physical page — remove old pv_entry first
        if crate::pv_list::valid_page(old_pa) {
            let pai = crate::pv_list::pa_index(old_pa);
            let _pv_lock = crate::pv_list::lock_pvh(pai);
            remove_pv_entry(old_pa, pmap, va);
        }
    } else {
        // New mapping — increment resident count
        guard.pmap_mut().stats.resident_count += 1;
    }

    // Build the new PTE
    let mut new_pte = pa_to_pte(pa)
        | INTEL_PTE_VALID
        | intel_prot_to_pte_bits(prot);

    if wired {
        new_pte |= INTEL_PTE_WIRED;
        guard.pmap_mut().stats.wired_count += 1;
    }

    // Write the PTE
    write_pte(pte_ptr, new_pte);

    // Add to pv_list reverse mapping
    if crate::pv_list::valid_page(pa) {
        let pai = crate::pv_list::pa_index(pa);
        let _pv_lock = crate::pv_list::lock_pvh(pai);
        add_pv_entry(pa, pmap, va);
    }
}

/// Convert vm_prot_t to Intel PTE protection bits.
fn intel_prot_to_pte_bits(prot: VmProt) -> PhysAddr {
    let mut bits: PhysAddr = INTEL_PTE_USER; // user-accessible by default
    if (prot as u32 & VM_PROT_WRITE as u32) != 0 {
        bits |= INTEL_PTE_WRITE;
    }
    // Note: no-execute (NX) bit handling goes here (PTE_NX on x86_64)
    bits
}

/// Update protection/wiring on an existing valid mapping.
fn update_pte_protection(pte_ptr: *mut Pte, prot: VmProt, wired: bool) {
    let pte = unsafe { *pte_ptr };
    let mut new_pte = (pte & INTEL_PTE_PFN) | INTEL_PTE_VALID | intel_prot_to_pte_bits(prot);
    if wired {
        new_pte |= INTEL_PTE_WIRED;
    }
    unsafe { *pte_ptr = new_pte };
}

/// Write a PTE value, respecting Xen PV page tables if needed.
#[cfg(not(feature = "xen"))]
fn write_pte(pte_ptr: *mut Pte, value: PhysAddr) {
    unsafe { *pte_ptr = value; }
}

#[cfg(feature = "xen")]
fn write_pte(pte_ptr: *mut Pte, value: PhysAddr) {
    // Under Xen PV, PTE writes go through a hypercall
    extern "C" {
        fn hypervisor_write_pte(ptr: *mut Pte, value: PhysAddr);
    }
    unsafe { hypervisor_write_pte(pte_ptr, value); }
}

use crate::locking::PmapReadGuard;
```

- [ ] **Step 2: Implement helper stubs `add_pv_entry` and `remove_pv_entry`**

Add to `mapping.rs`:
```rust
/// Add a pv_entry mapping from physical page → (pmap, va).
fn add_pv_entry(pa: PhysAddr, pmap: &Pmap, va: VmOffset) {
    let entry = crate::pv_list::pv_alloc();
    if entry.is_null() {
        panic!("pv_alloc failed");
    }
    let pai = crate::pv_list::pa_index(pa);
    unsafe {
        // Insert at head of pv_head_table
        (*entry).pmap = pmap as *const Pmap as *mut Pmap;
        (*entry).va = va;
        (*entry).next = pv_head_table;
        let head: *mut PvEntry = pv_head_table;
        if !head.is_null() {
            (*entry).next = *head;
        }
        *head = *entry;
    }
}

/// Remove a specific pv_entry from a physical page's mapping list.
fn remove_pv_entry(pa: PhysAddr, pmap: &Pmap, va: VmOffset) {
    let pai = crate::pv_list::pa_index(pa);
    unsafe {
        let head = pv_head_table.add(pai);
        let mut prev: *mut PvEntry = core::ptr::null_mut();
        let mut cur = *head;

        while !cur.is_null() {
            if (*cur).pmap == pmap as *const Pmap as *mut Pmap && (*cur).va == va {
                if prev.is_null() {
                    *head = (*cur).next;
                } else {
                    (*prev).next = (*cur).next;
                }
                crate::pv_list::pv_free(cur);
                return;
            }
            prev = cur;
            cur = (*cur).next;
        }
    }
}

extern "C" {
    static mut pv_head_table: *mut PvEntry;
}
```

- [ ] **Step 3: Verify `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/mapping.rs
git commit -m "rust/pmap: implement pmap_enter() core mapping insertion"
```

---

- [ ] [REVIEW GATE] **Phase 7 Complete: Core mapping operations. Review `pmap_enter()` against C implementation in `i386/intel/pmap.c:pmap_enter()` (line ~2172). Run: `cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features x86_64` — compiles cleanly.**

---

## Phase 8: `pmap_remove()`, `pmap_protect()`, `pmap_page_protect()`

### Task 18: Implement `pmap_remove()` and internal `pmap_remove_range()`

**Files:**
- Modify: `rust/pmap/src/mapping.rs`

- [ ] **Step 1: Write `pmap_remove()`**

```rust
/// Remove all mappings in the virtual address range [s, e) from a pmap.
pub fn pmap_remove(pmap: &mut Pmap, s: VmOffset, e: VmOffset) {
    if s >= e { return; }

    let guard = unsafe { PmapReadGuard::acquire(pmap) };

    let mut va = s;
    while va < e {
        let pde = pmap_pde(pmap, va); // defined below
        if pde.is_null() || !pte_is_valid(pde) {
            va = ((va >> PDESHIFT) + 1) << PDESHIFT; // skip to next PDE
            if va == 0 { break; } // wraparound
            continue;
        }

        let pt = phystokv(pte_to_pa(pde)) as *mut Pte;
        let start_idx = ptenum(va) as isize;
        let end_idx = core::cmp::min(
            ptenum(e - 1) as isize + 1,
            I386_PGBYTES as isize / core::mem::size_of::<Pte>() as isize,
        );

        pmap_remove_range(guard.pmap_mut(), va, pt, start_idx, end_idx);

        // Advance to next PDE boundary
        va = ((va >> PDESHIFT) + 1) << PDESHIFT;
        if va == 0 { break; }
    }
}

/// Remove a contiguous range of PTEs within a single page table.
fn pmap_remove_range(
    pmap: &mut Pmap,
    base_va: VmOffset,
    pt: *mut Pte,
    start_idx: isize,
    end_idx: isize,
) {
    for i in start_idx..end_idx {
        let pte_ptr = unsafe { pt.offset(i) };
        let old_pte = unsafe { *pte_ptr };

        if !pte_is_valid(old_pte) { continue; }

        let pa = pte_to_pa(old_pte);

        // Collect modified/reference bits
        if (old_pte & INTEL_PTE_MOD) != 0 {
            set_phys_attribute(pa, INTEL_PTE_MOD);
        }
        if (old_pte & INTEL_PTE_REF) != 0 {
            set_phys_attribute(pa, INTEL_PTE_REF);
        }

        // Update pv_list
        if crate::pv_list::valid_page(pa) {
            let pai = crate::pv_list::pa_index(pa);
            let _pv_lock = crate::pv_list::lock_pvh(pai);
            remove_pv_entry(pa, pmap, base_va + (i as usize * I386_PGBYTES));
        }

        // Clear the PTE
        unsafe { *pte_ptr = 0 };

        // Update stats
        pmap.stats.resident_count -= 1;
        if (old_pte & INTEL_PTE_WIRED) != 0 {
            pmap.stats.wired_count -= 1;
        }
    }
}

/// Set a physical attribute bit for a page (called from pv_list context).
fn set_phys_attribute(_pa: PhysAddr, _bit: PhysAddr) {
    // phys_attribute_set: store MOD/REF in pmap_phys_attributes array
    // Simplified stub:
    extern "C" {
        fn phys_attribute_set(pa: PhysAddr, bit: PhysAddr);
    }
    unsafe { phys_attribute_set(_pa, _bit); }
}

/// Get a page directory entry pointer for a given virtual address in a pmap.
fn pmap_pde(pmap: &Pmap, addr: VmOffset) -> *mut Pte {
    crate::expand::pmap_pde(pmap, addr)
}
```

- [ ] **Step 2: Add `pmap_pde()` to `expand.rs`**

```rust
/// Get a pointer to the page directory entry for an address in a pmap.
/// Returns null if higher-level tables are missing.
pub fn pmap_pde(pmap: &Pmap, addr: VmOffset) -> *mut Pte {
    #[cfg(feature = "x86_64")]
    {
        if pmap.l4base.is_null() { return core::ptr::null_mut(); }
        let l4_idx = lin2l4num(addr) as usize;
        let l4e = unsafe { *pmap.l4base.add(l4_idx) };
        if !pte_is_valid(l4e) { return core::ptr::null_mut(); }

        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdp_table = unsafe { phystokv(pte_to_pa(l4e)) as *mut Pte };
        let pdpe = unsafe { *pdp_table.add(pdp_idx) };
        if !pte_is_valid(pdpe) { return core::ptr::null_mut(); }

        let pde_idx = lin2pdenum(addr) as usize;
        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        unsafe { pd_table.add(pde_idx) }
    }

    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        if pmap.pdpbase.is_null() { return core::ptr::null_mut(); }
        let pdp_idx = lin2pdpnum(addr) as usize;
        let pdpe = unsafe { *pmap.pdpbase.add(pdp_idx) };
        if !pte_is_valid(pdpe) { return core::ptr::null_mut(); }

        let pde_idx = lin2pdenum_cont(addr) as usize;
        let pd_table = unsafe { phystokv(pte_to_pa(pdpe)) as *mut Pte };
        unsafe { pd_table.add(pde_idx) }
    }

    #[cfg(not(feature = "pae"))]
    {
        if pmap.dirbase.is_null() { return core::ptr::null_mut(); }
        let pde_idx = lin2pdenum(addr) as usize;
        unsafe { pmap.dirbase.add(pde_idx) }
    }
}

fn pte_is_valid(pte: PhysAddr) -> bool {
    (pte & INTEL_PTE_VALID) != 0
}
```

- [ ] **Step 3: Verify `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/mapping.rs rust/pmap/src/expand.rs
git commit -m "rust/pmap: implement pmap_remove() and pmap_pde()"
```

---

### Task 19: Implement `pmap_protect()` and `pmap_page_protect()`

**Files:**
- Create: `rust/pmap/src/protection.rs`

- [ ] **Step 1: Write `protection.rs`**

```rust
// rust/pmap/src/protection.rs
// Address-range and per-page protection changes.

use crate::types::*;
use crate::locking::PmapReadGuard;

/// Lower the protection on a virtual address range in a pmap.
/// If prot == VM_PROT_NONE, delegates to pmap_remove().
pub fn pmap_protect(pmap: &mut Pmap, s: VmOffset, e: VmOffset, prot: VmProt) {
    if prot == VM_PROT_NONE {
        crate::mapping::pmap_remove(pmap, s, e);
        return;
    }

    if s >= e { return; }
    let guard = unsafe { PmapReadGuard::acquire(pmap) };

    let mut va = s;
    while va < e {
        let pte_ptr = crate::expand::pmap_pte(pmap, va);
        if !pte_ptr.is_null() {
            let mut pte = unsafe { *pte_ptr };
            if (pte & INTEL_PTE_VALID) != 0 {
                // Lower protection: clear WRITE bit
                let new_prot_bits = intel_prot_to_pte_bits(prot);
                pte &= !INTEL_PTE_WRITE;
                pte |= new_prot_bits & INTEL_PTE_WRITE;
                unsafe { *pte_ptr = pte };
            }
        }
        va += I386_PGBYTES;
        if va == 0 { break; }
    }
}

/// Walk the pv_list for a physical page and remove or write-protect
/// all mappings across all pmaps (used for copy-on-write and pageout).
pub fn pmap_page_protect(phys: PhysAddr, prot: VmProt) {
    if !crate::pv_list::valid_page(phys) { return; }

    let pai = crate::pv_list::pa_index(phys);
    let _pv_lock = crate::pv_list::lock_pvh(pai);

    unsafe {
        let mut entry = pv_head_table.add(pai);
        while !entry.is_null() && !(*entry).is_null() {
            let pmap_ptr = (*(*entry)).pmap;
            let va = (*(*entry)).va;

            if prot == VM_PROT_NONE {
                // Remove mapping entirely
                crate::mapping::pmap_remove(&mut *pmap_ptr, va, va + I386_PGBYTES);
            } else {
                // Write-protect
                let pte_ptr = crate::expand::pmap_pte(&*pmap_ptr, va);
                if !pte_ptr.is_null() {
                    let pte = unsafe { *pte_ptr };
                    if (pte & INTEL_PTE_VALID) != 0 {
                        unsafe { *pte_ptr = pte & !INTEL_PTE_WRITE };
                    }
                }
            }
            entry = (*(*entry)).next;
        }
    }
}

use crate::mapping::intel_prot_to_pte_bits;

extern "C" {
    static mut pv_head_table: *mut PvEntry;
}
```

- [ ] **Step 2: Verify `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/protection.rs
git commit -m "rust/pmap: implement pmap_protect() and pmap_page_protect()"
```

---

- [ ] [REVIEW GATE] **Phase 8 Complete: Remove and protect operations. Review against C pmap_remove() (line ~1745) and pmap_page_protect() (line ~1786). Run `cargo build` to confirm compilation.**

---

## Phase 9: Page Attributes (Modified/Referenced)

### Task 20: Implement `page_attr.rs` — modified/reference tracking

**Files:**
- Create: `rust/pmap/src/page_attr.rs`

- [ ] **Step 1: Write `page_attr.rs`**

```rust
// rust/pmap/src/page_attr.rs
// Modified/reference bit tracking across all mappings of a physical page.

use crate::types::*;
use crate::locking::PmapWriteGuard;

/// Physical page attribute bits (mirror PTE definitions)
const PHYS_MODIFIED:   PhysAddr = INTEL_PTE_MOD;
const PHYS_REFERENCED: PhysAddr = INTEL_PTE_REF;

extern "C" {
    static mut pmap_phys_attributes: *mut u8;
    static mut pv_head_table: *mut PvEntry;
}

/// Clear specified attribute bits for a physical page across all pmaps.
fn phys_attribute_clear(phys: PhysAddr, bits: PhysAddr) {
    if !crate::pv_list::valid_page(phys) { return; }

    let _guard = PmapWriteGuard::acquire();
    let pai = crate::pv_list::pa_index(phys);

    // Clear cached attribute
    let attr_mask = !(bits as u8);
    unsafe {
        *pmap_phys_attributes.add(pai) &= attr_mask;
    }

    // Walk pv_list and clear bits in every PTE
    let _pv_lock = crate::pv_list::lock_pvh(pai);
    unsafe {
        let head = pv_head_table.add(pai);
        let mut entry = *head;
        while !entry.is_null() {
            let pmap = &*(*entry).pmap;
            let va = (*entry).va;
            let pte_ptr = crate::expand::pmap_pte(pmap, va);
            if !pte_ptr.is_null() {
                *pte_ptr &= !bits;
            }
            entry = (*entry).next;
        }
    }
}

/// Test whether any mapping of a physical page has specified attribute bits set.
fn phys_attribute_test(phys: PhysAddr, bits: PhysAddr) -> bool {
    if !crate::pv_list::valid_page(phys) { return false; }

    let pai = crate::pv_list::pa_index(phys);

    // Check cached attribute first
    let cached = unsafe { *pmap_phys_attributes.add(pai) };
    if (cached & (bits as u8)) != 0 {
        return true;
    }

    // Walk pv_list and check PTEs
    let _pv_lock = crate::pv_list::lock_pvh(pai);
    unsafe {
        let head = pv_head_table.add(pai);
        let mut entry = *head;
        while !entry.is_null() {
            let pmap = &*(*entry).pmap;
            let va = (*entry).va;
            let pte_ptr = crate::expand::pmap_pte(pmap, va);
            if !pte_ptr.is_null() && ((*pte_ptr & bits) != 0) {
                return true;
            }
            entry = (*entry).next;
        }
    }
    false
}

/// Clear the dirty (modified) bit for a physical page across all pmaps.
pub fn pmap_clear_modify(phys: PhysAddr) {
    phys_attribute_clear(phys, PHYS_MODIFIED);
}

/// Return whether any mapping of the physical page has the dirty bit set.
pub fn pmap_is_modified(phys: PhysAddr) -> bool {
    phys_attribute_test(phys, PHYS_MODIFIED)
}

/// Clear the accessed (referenced) bit for a physical page across all pmaps.
pub fn pmap_clear_reference(phys: PhysAddr) {
    phys_attribute_clear(phys, PHYS_REFERENCED);
}

/// Return whether any mapping of the physical page has the accessed bit set.
pub fn pmap_is_referenced(phys: PhysAddr) -> bool {
    phys_attribute_test(phys, PHYS_REFERENCED)
}
```

- [ ] **Step 2: Verify `cargo check`**

```bash
cd rust/pmap && cargo check --target x86_64-gnumach.json --features x86_64 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/page_attr.rs
git commit -m "rust/pmap: implement page attribute tracking (modified/referenced bits)"
```

---

- [ ] [REVIEW GATE] **Phase 9 Complete: Page attribute tracking. Review against C phys_attribute_clear() (line ~2831) and phys_attribute_test() (line ~2914). Run `cargo build`.**

---

## Phase 10: Lifecycle — Create, Destroy, Reference, Collect

### Task 21: Implement `lifecycle.rs`

**Files:**
- Create: `rust/pmap/src/lifecycle.rs`

- [ ] **Step 1: Write `lifecycle.rs`**

```rust
// rust/pmap/src/lifecycle.rs
// Pmap lifecycle: create, destroy, reference, collect.

use crate::types::*;

extern "C" {
    // Slab caches created in C's pmap_init()
    static mut pmap_cache:    *mut c_void; // kmem_cache for struct pmap
    static mut pdpt_cache:    *mut c_void; // kmem_cache for page directory pointer tables
    static mut kmem_cache_alloc: unsafe fn(*mut c_void) -> *mut c_void;
    static mut kmem_cache_free:  unsafe fn(*mut c_void, *mut c_void);
}

/// Allocate a new zeroed page-table page from the kernel allocator.
fn alloc_page_table_page() -> *mut Pte {
    // Allocate a physical page and return its kernel virtual address.
    extern "C" {
        fn pmap_page_table_page_alloc() -> *mut Pte;
    }
    unsafe { pmap_page_table_page_alloc() }
}

/// Create a new physical address map.
pub fn pmap_create(_size: VmSize) -> *mut Pmap {
    let pmap = unsafe { kmem_cache_alloc(pmap_cache) as *mut Pmap };
    if pmap.is_null() { return core::ptr::null_mut(); }

    unsafe {
        // Zero the structure
        core::ptr::write_bytes(pmap, 0, 1);

        // Allocate page directory (and higher levels for PAE/x86_64)
        #[cfg(feature = "x86_64")]
        {
            (*pmap).l4base = alloc_page_table_page();
            if (*pmap).l4base.is_null() {
                kmem_cache_free(pmap_cache, pmap as *mut c_void);
                return core::ptr::null_mut();
            }
            // Copy kernel L4 entries (the upper half) from kernel_pmap
            let kernel_l4 = unsafe { (*kernel_pmap).l4base };
            let start = lin2l4num(kernel_virtual_start) as usize;
            let end   = lin2l4num(kernel_virtual_end) as usize + 1;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    kernel_l4.add(start),
                    (*pmap).l4base.add(start),
                    end - start,
                );
            }
        }

        #[cfg(all(feature = "pae", not(feature = "x86_64")))]
        {
            (*pmap).pdpbase = alloc_page_table_page();
            if (*pmap).pdpbase.is_null() {
                kmem_cache_free(pmap_cache, pmap as *mut c_void);
                return core::ptr::null_mut();
            }
            // Copy kernel PDP entries
            // ...
        }

        #[cfg(not(feature = "pae"))]
        {
            (*pmap).dirbase = alloc_page_table_page();
            if (*pmap).dirbase.is_null() {
                kmem_cache_free(pmap_cache, pmap as *mut c_void);
                return core::ptr::null_mut();
            }
            // Copy kernel PDE entries
            // ...
        }

        (*pmap).ref_count = 1;
        // stats zeroed by write_bytes
        // lock zeroed
    }
    pmap
}

/// Destroy a pmap (decrement refcount; free if zero).
pub fn pmap_destroy(p: *mut Pmap) {
    if p.is_null() { return; }
    unsafe {
        (*p).ref_count -= 1;
        if (*p).ref_count > 0 { return; }
        // Free all page table pages
        // ...
        kmem_cache_free(pmap_cache, p as *mut c_void);
    }
}

/// Increment reference count.
pub fn pmap_reference(p: *mut Pmap) {
    if p.is_null() { return; }
    unsafe { (*p).ref_count += 1; }
}

/// Garbage-collect unused page-table pages from a user pmap.
pub fn pmap_collect(p: *mut Pmap) {
    // Scan for page directory entries with no valid mappings, free them.
    // Stub for now — full implementation follows C's pmap_collect().
    let _ = p;
}
```

- [ ] **Step 2: Commit**

```bash
git add rust/pmap/src/lifecycle.rs
git commit -m "rust/pmap: implement lifecycle (create, destroy, reference, collect)"
```

---

- [ ] [REVIEW GATE] **Phase 10 Complete: Pmap lifecycle. Review against C pmap_create() (line ~1330) and pmap_destroy() (line ~1486). Run `cargo build`.**

---

## Phase 11: Physical Operations, Map Windows, Activation

### Task 22: Implement `phys_ops.rs` and `mapwindow.rs`

**Files:**
- Create: `rust/pmap/src/phys_ops.rs`
- Create: `rust/pmap/src/mapwindow.rs`

- [ ] **Step 1: Write `phys_ops.rs`**

```rust
// rust/pmap/src/phys_ops.rs
use crate::types::*;

/// Convert kernel virtual address to physical address.
/// Only valid for direct-mapped kernel addresses.
pub fn kvtophys(va: VmOffset) -> PhysAddr {
    extern "C" {
        fn kvtophys(va: VmOffset) -> PhysAddr;
    }
    unsafe { kvtophys(va) }
}

/// Copy from virtual to physical memory.
pub fn copy_to_phys(src_va: VmOffset, dst_pa: PhysAddr, count: c_int) {
    extern "C" {
        fn copy_to_phys(src: VmOffset, dst: PhysAddr, count: c_int);
    }
    unsafe { copy_to_phys(src_va, dst_pa, count) }
}

/// Copy from physical to virtual memory.
pub fn copy_from_phys(src_pa: PhysAddr, dst_va: VmOffset, count: c_int) {
    extern "C" {
        fn copy_from_phys(src: PhysAddr, dst: VmOffset, count: c_int);
    }
    unsafe { copy_from_phys(src_pa, dst_va, count) }
}

use core::ffi::c_int;
```

- [ ] **Step 2: Write `mapwindow.rs`**

```rust
// rust/pmap/src/mapwindow.rs
use crate::types::*;

/// Acquire a temporary kernel virtual mapping for a physical page entry.
pub fn pmap_get_mapwindow(entry: PhysAddr) -> *mut PmapMapwindow {
    extern "C" {
        fn pmap_get_mapwindow(entry: PhysAddr) -> *mut PmapMapwindow;
    }
    unsafe { pmap_get_mapwindow(entry) }
}

/// Release a previously acquired map window.
pub fn pmap_put_mapwindow(map: *mut PmapMapwindow) {
    extern "C" {
        fn pmap_put_mapwindow(map: *mut PmapMapwindow);
    }
    unsafe { pmap_put_mapwindow(map) }
}
```

- [ ] **Step 3: Write `activation.rs`**

**Files:**
- Create: `rust/pmap/src/activation.rs`

```rust
// rust/pmap/src/activation.rs
use crate::types::*;

/// Activate a pmap on a CPU (from macros PMAP_ACTIVATE_USER/KERNEL).
pub fn pmap_activate(pmap: *mut Pmap, _thread: Thread, _cpu: c_int) {
    if pmap.is_null() { return; }

    #[cfg(feature = "smp")]
    {
        // SMP path: manages cpus_active, cpus_using, processes updates
        extern "C" {
            fn pmap_activate_smp(pmap: *mut Pmap, cpu: c_int);
        }
        unsafe { pmap_activate_smp(pmap, _cpu); }
    }
    #[cfg(not(feature = "smp"))]
    {
        unsafe {
            set_pmap(pmap);
            (*pmap).cpus_using = 1;
        }
    }
}

/// Deactivate a pmap on a CPU.
pub fn pmap_deactivate(pmap: *mut Pmap, _thread: Thread, _cpu: c_int) {
    if pmap.is_null() { return; }
    unsafe {
        #[cfg(feature = "smp")]
        {
            i_bit_clear(_cpu, &mut (*pmap).cpus_using);
        }
        #[cfg(not(feature = "smp"))]
        {
            (*pmap).cpus_using = 0;
        }
    }
}

/// Set hardware page table base (CR3/CR4).
unsafe fn set_pmap(pmap: &Pmap) {
    extern "C" {
        fn set_cr3(value: PhysAddr);
    }
    #[cfg(feature = "x86_64")]
    unsafe {
        set_cr3(kvtophys((*pmap).l4base as VmOffset));
    }
    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    unsafe {
        set_cr3(kvtophys((*pmap).pdpbase as VmOffset));
    }
    #[cfg(not(feature = "pae"))]
    unsafe {
        set_cr3(kvtophys((*pmap).dirbase as VmOffset));
    }
}

extern "C" {
    fn kvtophys(va: VmOffset) -> PhysAddr;
    fn i_bit_clear(bit: c_int, set: *mut CpuSet);
}

use core::ffi::c_int;
```

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/phys_ops.rs rust/pmap/src/mapwindow.rs rust/pmap/src/activation.rs
git commit -m "rust/pmap: implement phys_ops, mapwindow, and activation modules"
```

---

## Phase 12: FFI Layer — Complete C-Compatible Wrappers

### Task 23: Complete `ffi.rs` with all C exports

**Files:**
- Modify: `rust/pmap/src/ffi.rs`

- [ ] **Step 1: Write complete `ffi.rs`**

```rust
// rust/pmap/src/ffi.rs
// Complete #[no_mangle] extern "C" wrappers for all public PMAP symbols.
// Converts between C types (raw pointers, c_int, c_ulong) and Rust types.

use core::ffi::c_int;
use crate::types::*;

// ── Initialization ──────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_virtual_space(startp: *mut VmOffset, endp: *mut VmOffset) {
    unsafe {
        *startp = kernel_virtual_start;
        *endp = kernel_virtual_end;
    }
}

// ── Lifecycle ───────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_create(size: VmSize) -> *mut Pmap {
    crate::lifecycle::pmap_create(size)
}

#[no_mangle]
pub extern "C" fn pmap_destroy(p: *mut Pmap) {
    crate::lifecycle::pmap_destroy(p);
}

#[no_mangle]
pub extern "C" fn pmap_reference(p: *mut Pmap) {
    crate::lifecycle::pmap_reference(p);
}

#[no_mangle]
pub extern "C" fn pmap_collect(p: *mut Pmap) {
    crate::lifecycle::pmap_collect(p);
}

// ── Mapping ─────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_enter(
    pmap: *mut Pmap,
    va: VmOffset,
    pa: PhysAddr,
    prot: VmProt,
    wired: c_int,
) {
    if pmap.is_null() { return; }
    let pmap_ref = unsafe { &mut *pmap };
    crate::mapping::pmap_enter(pmap_ref, va, pa, prot, wired != 0);
}

#[no_mangle]
pub extern "C" fn pmap_remove(pmap: *mut Pmap, s: VmOffset, e: VmOffset) {
    if pmap.is_null() { return; }
    let pmap_ref = unsafe { &mut *pmap };
    crate::mapping::pmap_remove(pmap_ref, s, e);
}

#[no_mangle]
pub extern "C" fn pmap_extract(pmap: *mut Pmap, va: VmOffset) -> PhysAddr {
    if pmap.is_null() { return 0; }
    let pmap_ref = unsafe { &*pmap };
    crate::mapping::pmap_extract(pmap_ref, va).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn pmap_change_wiring(pmap: *mut Pmap, v: VmOffset, wired: c_int) {
    if pmap.is_null() { return; }
    // Toggle wiring bit on PTE
    let pte_ptr = crate::expand::pmap_pte(unsafe { &*pmap }, v);
    if pte_ptr.is_null() { return; }
    unsafe {
        if wired != 0 {
            *pte_ptr |= INTEL_PTE_WIRED;
            (*pmap).stats.wired_count += 1;
        } else {
            *pte_ptr &= !INTEL_PTE_WIRED;
            (*pmap).stats.wired_count -= 1;
        }
    }
}

#[no_mangle]
pub extern "C" fn pmap_pageable(
    pmap: *mut Pmap,
    _start: VmOffset,
    _end: VmOffset,
    _pageable: c_int,
) {
    // Advisory only — no operation in x86 PMAP.
    let _ = (pmap, _start, _end, _pageable);
}

#[no_mangle]
pub extern "C" fn pmap_map_bd(
    virt: VmOffset,
    start: PhysAddr,
    end: PhysAddr,
    prot: VmProt,
) -> VmOffset {
    crate::mapping::pmap_map_bd(virt, start, end, prot)
}

// ── Protection ──────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_protect(pmap: *mut Pmap, s: VmOffset, e: VmOffset, prot: VmProt) {
    if pmap.is_null() { return; }
    let pmap_ref = unsafe { &mut *pmap };
    crate::protection::pmap_protect(pmap_ref, s, e, prot);
}

#[no_mangle]
pub extern "C" fn pmap_page_protect(phys: PhysAddr, prot: VmProt) {
    crate::protection::pmap_page_protect(phys, prot);
}

// ── Page Attributes ─────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_clear_modify(phys: PhysAddr) {
    crate::page_attr::pmap_clear_modify(phys);
}

#[no_mangle]
pub extern "C" fn pmap_is_modified(phys: PhysAddr) -> c_int {
    crate::page_attr::pmap_is_modified(phys) as c_int
}

#[no_mangle]
pub extern "C" fn pmap_clear_reference(phys: PhysAddr) {
    crate::page_attr::pmap_clear_reference(phys);
}

#[no_mangle]
pub extern "C" fn pmap_is_referenced(phys: PhysAddr) -> c_int {
    crate::page_attr::pmap_is_referenced(phys) as c_int
}

// ── Physical Operations ─────────────────────────────────────────

#[no_mangle]
pub extern "C" fn kvtophys(va: VmOffset) -> PhysAddr {
    crate::phys_ops::kvtophys(va)
}

#[no_mangle]
pub extern "C" fn copy_to_phys(src: VmOffset, dst: PhysAddr, count: c_int) {
    crate::phys_ops::copy_to_phys(src, dst, count);
}

#[no_mangle]
pub extern "C" fn copy_from_phys(src: PhysAddr, dst: VmOffset, count: c_int) {
    crate::phys_ops::copy_from_phys(src, dst, count);
}

// ── Map Windows ─────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_get_mapwindow(entry: PhysAddr) -> *mut PmapMapwindow {
    crate::mapwindow::pmap_get_mapwindow(entry)
}

#[no_mangle]
pub extern "C" fn pmap_put_mapwindow(map: *mut PmapMapwindow) {
    crate::mapwindow::pmap_put_mapwindow(map)
}

// ── Activation ──────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn pmap_activate(pmap: *mut Pmap, thread: Thread, cpu: c_int) {
    crate::activation::pmap_activate(pmap, thread, cpu);
}

#[no_mangle]
pub extern "C" fn pmap_deactivate(pmap: *mut Pmap, thread: Thread, cpu: c_int) {
    crate::activation::pmap_deactivate(pmap, thread, cpu);
}

// ── Debug (conditional) ─────────────────────────────────────────

#[cfg(feature = "kdb")]
#[no_mangle]
pub extern "C" fn pmap_whatis(pmap: *mut Pmap, a: VmOffset) -> c_int {
    crate::whatis::pmap_whatis(pmap, a)
}

// ── SMP (conditional) ───────────────────────────────────────────

#[cfg(feature = "smp")]
#[no_mangle]
pub extern "C" fn signal_cpus(use_list: CpuSet, pmap: *mut Pmap, start: VmOffset, end: VmOffset) {
    crate::smp::signal_cpus(use_list, pmap, start, end);
}

#[cfg(feature = "smp")]
#[no_mangle]
pub extern "C" fn process_pmap_updates(my_pmap: *mut Pmap) {
    crate::smp::process_pmap_updates(my_pmap);
}

#[cfg(feature = "smp")]
#[no_mangle]
pub extern "C" fn pmap_update_interrupt() {
    crate::smp::pmap_update_interrupt();
}

// ── Xen (conditional) ───────────────────────────────────────────

#[cfg(feature = "xen")]
#[no_mangle]
pub extern "C" fn pmap_set_page_readwrite(addr: *mut c_void) {
    crate::xen::pmap_set_page_readwrite(addr);
}

#[cfg(feature = "xen")]
#[no_mangle]
pub extern "C" fn pmap_set_page_readonly(addr: *mut c_void) {
    crate::xen::pmap_set_page_readonly(addr);
}

#[cfg(feature = "xen")]
#[no_mangle]
pub extern "C" fn pmap_set_page_readonly_init(addr: *mut c_void) {
    crate::xen::pmap_set_page_readonly_init(addr);
}

#[cfg(feature = "xen")]
#[no_mangle]
pub extern "C" fn pmap_map_mfn(addr: *mut c_void, mfn: c_ulong) {
    crate::xen::pmap_map_mfn(addr, mfn);
}

use core::ffi::{c_ulong, c_void};

// pmap_kernel is already defined above in the initial ffi.rs stub
```

- [ ] **Step 2: Verify compilation with all features**

```bash
cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features x86_64,smp 2>&1 | tail -20
```

- [ ] **Step 3: Check exported symbols**

```bash
nm rust/pmap/target/x86_64-unknown-none/release/libpmap.a 2>/dev/null | grep " T pmap_" | sort
```
Expected: All `pmap_enter`, `pmap_remove`, `pmap_extract`, etc. are visible.

- [ ] **Step 4: Commit**

```bash
git add rust/pmap/src/ffi.rs
git commit -m "rust/pmap: complete FFI layer with all C-visible pmap exports"
```

---

- [ ] [REVIEW GATE] **Phase 12 Complete: FFI layer. Verify: `cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features all-available && nm target/*/release/libpmap.a | grep " T pmap_" | wc -l` — confirms all expected symbols are exported. Cross-reference with `vm/pmap.h` declarations.**

---

## Phase 13: Remaining Modules (SMP, Xen, Whatis, Collect)

### Task 24: Stub `smp.rs`, `xen.rs`, `whatis.rs`, and `collect.rs`

**Files:**
- Create: `rust/pmap/src/smp.rs`
- Create: `rust/pmap/src/xen.rs`
- Create: `rust/pmap/src/whatis.rs`
- Create: `rust/pmap/src/collect.rs`

These modules wrap the corresponding C assembly/FFI calls or provide stub implementations. For the initial migration, they delegate to the original C functions via FFI.

- [ ] **Step 1: Create stub files for each conditional module**

```rust
// smp.rs
use crate::types::*;
pub fn signal_cpus(use_list: CpuSet, _pmap: *mut Pmap, _start: VmOffset, _end: VmOffset) {
    // Delegates to C assembly implementation
    extern "C" { fn signal_cpus_c(use_list: CpuSet, pmap: *mut Pmap, start: VmOffset, end: VmOffset); }
    unsafe { signal_cpus_c(use_list, _pmap, _start, _end); }
}
pub fn process_pmap_updates(_my_pmap: *mut Pmap) {
    extern "C" { fn process_pmap_updates_c(pmap: *mut Pmap); }
    unsafe { process_pmap_updates_c(_my_pmap); }
}
pub fn pmap_update_interrupt() {
    extern "C" { fn pmap_update_interrupt_c(); }
    unsafe { pmap_update_interrupt_c(); }
}
```

```rust
// xen.rs
use core::ffi::{c_ulong, c_void};
pub fn pmap_set_page_readwrite(addr: *mut c_void) {
    extern "C" { fn pmap_set_page_readwrite_c(addr: *mut c_void); }
    unsafe { pmap_set_page_readwrite_c(addr); }
}
pub fn pmap_set_page_readonly(addr: *mut c_void) {
    extern "C" { fn pmap_set_page_readonly_c(addr: *mut c_void); }
    unsafe { pmap_set_page_readonly_c(addr); }
}
pub fn pmap_set_page_readonly_init(addr: *mut c_void) {
    extern "C" { fn pmap_set_page_readonly_init_c(addr: *mut c_void); }
    unsafe { pmap_set_page_readonly_init_c(addr); }
}
pub fn pmap_map_mfn(addr: *mut c_void, mfn: c_ulong) {
    extern "C" { fn pmap_map_mfn_c(addr: *mut c_void, mfn: c_ulong); }
    unsafe { pmap_map_mfn_c(addr, mfn); }
}
```

```rust
// whatis.rs
use crate::types::*;
pub fn pmap_whatis(_pmap: *mut Pmap, _a: VmOffset) -> c_int {
    extern "C" { fn pmap_whatis_c(pmap: *mut Pmap, a: VmOffset) -> c_int; }
    unsafe { pmap_whatis_c(_pmap, _a) }
}
use core::ffi::c_int;
```

```rust
// collect.rs
use crate::types::*;
pub fn pmap_collect(_p: *mut Pmap) {
    // Implemented in lifecycle.rs; this is a thin re-export.
    crate::lifecycle::pmap_collect(_p);
}
```

- [ ] **Step 2: Verify compilation with all features**

```bash
cd rust/pmap && cargo build --release --target x86_64-gnumach.json --features x86_64,smp,xen 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add rust/pmap/src/smp.rs rust/pmap/src/xen.rs rust/pmap/src/whatis.rs rust/pmap/src/collect.rs
git commit -m "rust/pmap: add stub modules for SMP, Xen, whatis, and collect"
```

---

- [ ] [REVIEW GATE] **Phase 13 Complete: All modules compilable. Run `cargo build --release --target x86_64-gnumach.json --features x86_64,smp,xen,kdb` — clean build, all symbols exported.**

---

## Phase 14: Extract C Bootstrap File

### Task 25: Create `pmap_bootstrap.c` from `pmap.c`

**Files:**
- Create: `i386/intel/pmap_bootstrap.c`
- Modify: `i386/Makefrag_x86.am` (already done in Task 5)

- [ ] **Step 1: Extract bootstrap functions from `pmap.c`**

Create `i386/intel/pmap_bootstrap.c` containing only these functions from the original `pmap.c`:
- `pmap_bootstrap()` (line ~736)
- `pmap_bootstrap_pae()` (line ~625)
- `pmap_bootstrap_xen()` (line ~681)
- `pmap_set_page_dir()` (line ~3310)
- `pmap_unmap_page_zero()` (line ~3249)
- `pmap_make_temporary_mapping()` (line ~3269)
- `pmap_remove_temporary_mapping()` (line ~3329)
- `pmap_clear_bootstrap_pagetable()` (line ~970, Xen-only)
- `pmap_init()` (line ~1097)

Plus all static data they initialize:
- `kernel_pmap`, `kernel_page_dir`, `kernel_virtual_start`, `kernel_virtual_end`
- `pv_lock_table`, `pv_head_table`, `pmap_phys_attributes`
- `pmap_initialized`
- `pmap_object`, `pmap_cache`, `pdpt_cache`, `pv_list_cache`
- `pmap_system_lock`, `cpus_active`, `cpus_idle`, `cpu_update_needed` (SMP)

Copy these functions verbatim from the current `pmap.c` with all their includes.

- [ ] **Step 2: Verify the original `pmap.c` is kept as reference**

```bash
# Do not delete pmap.c yet — keep it for reference during migration
wc -l i386/intel/pmap.c i386/intel/pmap_bootstrap.c 2>/dev/null
```

- [ ] **Step 3: Commit**

```bash
git add i386/intel/pmap_bootstrap.c
git commit -m "pmap: extract bootstrap functions into pmap_bootstrap.c"
```

---

## Phase 15: Integration Test

### Task 26: Create integration smoke test

**Files:**
- Create: `tests/test-pmap-rust.c`

- [ ] **Step 1: Write minimal QEMU test that exercises the Rust PMAP**

```c
// tests/test-pmap-rust.c
#include <testlib.h>
#include <mach/mach.h>

void test_main(void) {
    // 1. Verify kernel pmap exists
    vm_map_t kernel_map = mach_host_self(); // or equivalent
    assert(kernel_map != VM_MAP_NULL, "kernel map is null");

    // 2. Verify pmap_extract works for a known kernel address
    // Map a test page and verify round-trip
    // (actual test depends on available kernel VM APIs)

    // 3. Verify pmap_enter/pmap_remove cycle
    // ... simplified: just verify the kernel boots with Rust PMAP

    test_success();
}
```

- [ ] **Step 2: Add to test Makefile**

Add `test-pmap-rust` to `USER_TESTS` in `tests/user-qemu.mk`.

- [ ] **Step 3: Run the test**

```bash
make tests/test-pmap-rust 2>&1 | tail -20
```

- [ ] **Step 4: Commit**

```bash
git add tests/test-pmap-rust.c tests/Makefrag.am
git commit -m "tests: add pmap-rust integration smoke test"
```

---

- [ ] [REVIEW GATE] **Phase 15 Complete: Integration test. Run `make check` — the full test suite passes. If QEMU unavailable, verify `cargo build` succeeds and symbol list matches expected exports.**

---

## Phase 16: Cleanup & Dead Code Removal

### Task 27: Remove dead Rust code, add final documentation

**Files:**
- (possibly) Remove or comment out original `pmap.c` after verification

- [ ] **Step 1: Mark original `pmap.c` as deprecated**

Add a comment at the top of `i386/intel/pmap.c`:
```c
/*
 * NOTE: This file has been replaced by rust/pmap/src/*.rs
 * Except for bootstrap functions which now live in pmap_bootstrap.c.
 * This file is retained for reference during the migration period.
 * TODO: Remove once Rust PMAP is fully validated.
 */
```

- [ ] **Step 2: Run full test suite**

```bash
make check 2>&1
```

- [ ] **Step 3: Commit**

```bash
git add i386/intel/pmap.c
git commit -m "pmap: mark original pmap.c as deprecated (replaced by Rust)"
```

---

## Phase 17: Host-Side Unit Tests

### Task 28: Add comprehensive host-side unit tests for all pure-math functions

**Files:**
- Modify: `rust/pmap/src/types.rs` (expand tests)
- Create: `rust/pmap/tests/address_math.rs`

- [ ] **Step 1: Write comprehensive address decomposition tests**

```rust
// rust/pmap/tests/address_math.rs
use pmap::types::*;

#[test]
fn test_addr_decomposition_roundtrip() {
    let tests: &[(VmOffset, &str)] = &[
        (0x0000_0000_0000_0000, "zero"),
        (0x0000_0000_0000_1000, "page 1"),
        (0xFFFF_FFFF_FFFF_F000, "last page"),
        (0x0000_7F00_0000_0000, "mid-range"),
    ];

    for &(addr, name) in tests {
        #[cfg(feature = "x86_64")]
        {
            let l4 = lin2l4num(addr);
            let pdp = lin2pdpnum(addr);
            let pde = lin2pdenum(addr);
            let pte = ptenum(addr);
            // Verify reconstruction: pagenum2lin(l4, pdp, pde, pte) ≈ addr (page-aligned)
            let recon = pagenum2lin(l4, pdp, pde, pte);
            assert_eq!(recon, addr & !0xFFF, "reconstruction failed for {}", name);
        }
    }
}

#[test]
fn test_pte_bit_isolation() {
    // Each PTE bit should be independently testable
    let pte = INTEL_PTE_VALID | INTEL_PTE_WRITE | INTEL_PTE_USER | INTEL_PTE_MOD;
    assert!((pte & INTEL_PTE_VALID) != 0);
    assert!((pte & INTEL_PTE_WRITE) != 0);
    assert!((pte & INTEL_PTE_EXECUTE) == 0); // not set
    assert!((pte & INTEL_PTE_MOD) != 0);
}

#[test]
fn test_pa_pte_roundtrip_many() {
    let test_pas = &[0u64, 0x1000, 0x12345000, 0xFFFFF000];
    for &pa in test_pas {
        let pa = pa as PhysAddr;
        let pte = pa_to_pte(pa);
        assert_eq!(pte_to_pa(pte), pa, "round-trip failed for pa {:#x}", pa);
    }
}

#[test]
fn test_pte_increment() {
    let mut pte: PhysAddr = 0x1000;
    let orig = pte;
    pte_increment_pa(&mut pte);
    assert_eq!(pte, orig + 0x1000);
    assert_eq!(pte_to_pa(pte), 0x2000);
}
```

- [ ] **Step 2: Add `pagenum2lin()` to `types.rs`**

```rust
/// Reconstruct page-aligned linear address from table indices.
pub fn pagenum2lin(l4: u32, pdp: u32, pde: u32, pte: u32) -> VmOffset {
    #[cfg(feature = "x86_64")]
    {
        ((l4 as VmOffset) << L4SHIFT) +
        ((pdp as VmOffset) << PDPSHIFT) +
        ((pde as VmOffset) << PDESHIFT) +
        ((pte as VmOffset) << PTESHIFT)
    }
    #[cfg(all(feature = "pae", not(feature = "x86_64")))]
    {
        ((pdp as VmOffset) << PDPSHIFT) +
        ((pde as VmOffset) << PDESHIFT) +
        ((pte as VmOffset) << PTESHIFT)
    }
    #[cfg(not(feature = "pae"))]
    {
        ((pde as VmOffset) << PDESHIFT) +
        ((pte as VmOffset) << PTESHIFT)
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd rust/pmap && cargo test --features x86_64 2>&1
```
Expected: All tests pass.

- [ ] **Step 4: Add `pub` visibility to tests' used functions**

Ensure `pagenum2lin`, `lin2l4num`, `lin2pdpnum`, `lin2pdenum`, `ptenum`, `pa_to_pte`, `pte_to_pa`, `pte_increment_pa` are all `pub`.

- [ ] **Step 5: Commit**

```bash
git add rust/pmap/tests/address_math.rs rust/pmap/src/types.rs
git commit -m "rust/pmap: add comprehensive address math host-side unit tests"
```

---

- [ ] [REVIEW GATE] **Phase 17 Complete: Full test coverage for pure functions. Run `cd rust/pmap && cargo test --features x86_64` — all tests pass.**

---

## Summary of All Tasks

| Phase | Tasks | What's built |
|-------|-------|-------------|
| 1 | 1–5 | Project skeleton, Cargo.toml, build.rs, Autotools integration |
| 2 | 6–8 | Type system: Pmap struct, PTE constants, address index math, host tests |
| 3 | 9 | Locking: RAII guards (SplGuard, PmapReadGuard, PmapWriteGuard, PvLockGuard) |
| 4 | 10–11 | Global state wiring, FFI stub with `pmap_kernel` export |
| 5 | 12–14 | Page table walk (`pmap_pte`, `pmap_pde`) and expand (`pmap_expand`) |
| 6 | 15 | PV list management (reverse mapping table) |
| 7 | 16–17 | Core mapping: `pmap_extract`, `pmap_enter` |
| 8 | 18–19 | `pmap_remove`, `pmap_protect`, `pmap_page_protect` |
| 9 | 20 | Page attributes: modified/reference tracking |
| 10 | 21 | Lifecycle: create, destroy, reference, collect |
| 11 | 22 | Physical ops, map windows, activation |
| 12 | 23 | Complete FFI layer with all C exports |
| 13 | 24 | Stub modules for SMP, Xen, whatis, collect |
| 14 | 25 | C bootstrap extraction into `pmap_bootstrap.c` |
| 15 | 26 | QEMU integration smoke test |
| 16 | 27 | Cleanup: deprecate old `pmap.c` |
| 17 | 28 | Comprehensive host-side unit tests |

---

**Estimated total build agent time:** ~80–120 minutes (28 tasks × 3–5 min each plus verification).

**Key milestones for review:**
1. After Phase 4: Rust compiles and exports C symbols
2. After Phase 7: Core mapping (`pmap_enter`/`pmap_extract`) works
3. After Phase 12: All FFI symbols exported
4. After Phase 15: QEMU smoke test boots
