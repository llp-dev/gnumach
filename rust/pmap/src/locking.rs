// rust/pmap/src/locking.rs — RAII lock guards for PMAP operations.
//
// Enforces the 3-level locking protocol:
//  1. pmap-only ops: lock only the pmap
//  2. pmap-based ops: SPLVM → pmap_system_lock(read) → simple_lock(pmap)
//  3. pv_list-based ops: SPLVM → pmap_system_lock(write) → pv_list_lock
//
// On UP (#[cfg(not(smp))]), guards are zero-cost no-ops.
// On SMP (#[cfg(smp)]), guards manage SPLVM, cpus_active bitmap, and locks.

use core::ffi::{c_int, c_void};
use crate::types::{Pmap, SimpleLock, CpuSet};

//
// External C functions — called via FFI at the unsafe boundary.
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

    // Global state (allocated in C's pmap_bootstrap.c)
    #[cfg(feature = "smp")]
    static mut pmap_system_lock: c_void;
    #[cfg(feature = "smp")]
    static mut cpus_active: CpuSet;
}

//
// SplGuard — raises SPLVM, removes CPU from cpus_active on SMP.
// Restores on Drop.
//
#[cfg(feature = "smp")]
pub struct SplGuard {
    prev_spl: c_int,
}

#[cfg(feature = "smp")]
impl SplGuard {
    /// Acquire SPLVM and remove this CPU from cpus_active.
    ///
    /// # Safety
    /// Must not be called from interrupt context at or above SPLVM.
    pub unsafe fn acquire() -> Self {
        let spl = splvm();
        i_bit_clear(cpu_number(), core::ptr::addr_of_mut!(cpus_active));
        SplGuard { prev_spl: spl }
    }
}

#[cfg(feature = "smp")]
impl Drop for SplGuard {
    fn drop(&mut self) {
        unsafe {
            i_bit_set(cpu_number(), core::ptr::addr_of_mut!(cpus_active));
            splx(self.prev_spl);
        }
    }
}

#[cfg(not(feature = "smp"))]
pub struct SplGuard;

#[cfg(not(feature = "smp"))]
impl SplGuard {
    pub fn acquire() -> Self { SplGuard }
}

//
// PmapReadGuard — protects pmap-based operations.
// Acquires: SPLVM → read-lock pmap_system_lock → simple_lock(pmap).
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
    /// `pmap` must be a valid, initialized pmap. Must not be called at SPLVM.
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
        // _spl drops here → splx + cpus_active restore (on SMP)
    }
}

//
// PmapWriteGuard — exclusive write lock on pmap_system_lock.
// Used for pv_list-based operations (pmap_remove_all, pmap_copy_on_write).
//
#[cfg(feature = "smp")]
pub struct PmapWriteGuard {
    _spl: SplGuard,
}

#[cfg(feature = "smp")]
impl PmapWriteGuard {
    /// Acquire exclusive write lock on the pmap system.
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

#[cfg(not(feature = "smp"))]
pub struct PmapWriteGuard;

#[cfg(not(feature = "smp"))]
impl PmapWriteGuard {
    pub fn acquire() -> Self { PmapWriteGuard }
}

//
// PvLockGuard — bit lock on a pv_head_table entry.
// On SMP: locks the bit. On UP: no-op.
//
#[cfg(feature = "smp")]
pub struct PvLockGuard {
    index: usize,
}

#[cfg(feature = "smp")]
impl PvLockGuard {
    /// Lock a pv_head_table entry by index.
    pub unsafe fn acquire(index: usize) -> Self {
        extern "C" { fn bit_lock(bit: usize, table: *mut u8); }
        extern "C" { static mut pv_lock_table: *mut u8; }
        bit_lock(index, pv_lock_table);
        PvLockGuard { index }
    }
}

#[cfg(feature = "smp")]
impl Drop for PvLockGuard {
    fn drop(&mut self) {
        unsafe {
            extern "C" { fn bit_unlock(bit: usize, table: *mut u8); }
            extern "C" { static mut pv_lock_table: *mut u8; }
            bit_unlock(self.index, pv_lock_table);
        }
    }
}

#[cfg(not(feature = "smp"))]
pub struct PvLockGuard;

#[cfg(not(feature = "smp"))]
impl PvLockGuard {
    pub fn acquire(_index: usize) -> Self { PvLockGuard }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_guard_drop_compiles() {
        // On UP, guard is a no-op. Verify it can be constructed and dropped.
        let guard = PmapWriteGuard::acquire();
        drop(guard);
    }

    #[test]
    fn test_pv_lock_guard_drop_compiles() {
        // On UP, guard is a no-op.
        let guard = PvLockGuard::acquire(0);
        drop(guard);
    }
}
