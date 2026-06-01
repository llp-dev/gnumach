// rust/pmap/src/locking.rs — Simplified RAII lock guards for UP kernel.
//
// On UP (non-SMP), all lock operations are no-ops.
// The kernel runs at elevated IPL during PMAP operations.
// On SMP (feature = "smp"), guards would manage SPLVM + cpus_active + locks.

use core::ffi::{c_int, c_void};
use crate::types::{Pmap, SimpleLock, CpuSet};

//
// SplGuard — no-op on UP, manages SPLVM on SMP.
//
pub struct SplGuard;

impl SplGuard {
    pub fn acquire() -> Self { SplGuard }
}

//
// PmapReadGuard — protects pmap-based operations.
// On UP: just holds a mutable reference to the pmap.
//
pub struct PmapReadGuard<'a> {
    pmap: &'a mut Pmap,
}

impl<'a> PmapReadGuard<'a> {
    /// Acquire pmap lock. On UP, this is a no-op for the pmap lock itself.
    ///
    /// # Safety
    /// `pmap` must be a valid, initialized pmap.
    pub unsafe fn acquire(pmap: &'a mut Pmap) -> Self {
        PmapReadGuard { pmap }
    }

    pub fn pmap(&self) -> &Pmap { self.pmap }
    pub fn pmap_mut(&mut self) -> &mut Pmap { self.pmap }
}

//
// PmapWriteGuard — exclusive write guard.
// On UP: no-op.
//
pub struct PmapWriteGuard;

impl PmapWriteGuard {
    pub fn acquire() -> Self { PmapWriteGuard }
}

//
// PvLockGuard — bit lock on pv_head_table entry.
// On UP: no-op.
//
pub struct PvLockGuard;

impl PvLockGuard {
    pub fn acquire(_index: usize) -> Self { PvLockGuard }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guards_drop_compile() {
        let guard = PmapWriteGuard::acquire();
        drop(guard);
        let pv = PvLockGuard::acquire(0);
        drop(pv);
    }
}
