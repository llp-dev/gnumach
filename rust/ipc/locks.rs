//! Shared lock + state primitives used across the IPC port.
//!
//! With NCPUS == 1 the `simple_lock`-based primitives (`ip_*`, `io_*`,
//! `imq_*`, `ips_*`) all collapse to no-ops; the read-write `lock_*` calls
//! (used by the per-space `is_*_lock` family) remain real because
//! `kern/lock.c::lock_{read,write,done}` are real function bodies even at
//! NCPUS == 1.  This module is the single source of truth so a future
//! NCPUS > 1 build only needs to touch one file.
//!
//! Some primitives (e.g. `io_lock_try`, `imq_lock_init`) are exported here
//! for completeness even when no current caller uses them, so that the
//! module is a faithful mirror of the C lock-API surface.
#![allow(dead_code)]

use core::ptr::addr_of_mut;

use crate::extern_c::{kmem_cache_free, lock_done, lock_read, lock_write};
use crate::mach_types::{
    ipc_mqueue, ipc_object, ipc_port, ipc_pset_t, ipc_space_t, vm_offset_t,
    IO_BITS_OTYPE,
};

// ---------------------------------------------------------------------------
//  Object locks (simple_lock — no-op at NCPUS == 1)
// ---------------------------------------------------------------------------

#[inline]
pub(crate) unsafe fn io_lock(_io: *mut ipc_object) {}
#[inline]
pub(crate) unsafe fn io_unlock(_io: *mut ipc_object) {}
#[inline]
pub(crate) unsafe fn io_lock_init(_io: *mut ipc_object) {}
#[inline]
pub(crate) unsafe fn io_lock_try(_io: *mut ipc_object) -> bool { true }

#[inline]
pub(crate) unsafe fn io_active(io: *mut ipc_object) -> bool {
    ((*io).io_bits as i32) < 0
}

#[inline]
pub(crate) unsafe fn io_reference(io: *mut ipc_object) {
    (*io).io_references += 1;
}

#[inline]
pub(crate) unsafe fn io_release(io: *mut ipc_object) {
    (*io).io_references -= 1;
}

/// `io_check_unlock(io)` — release lock + if refs hit zero, free into the
/// type-appropriate cache.  Mirrors the inline in `ipc/ipc_object.h`.
#[inline]
pub(crate) unsafe fn io_check_unlock(io: *mut ipc_object) {
    let refs = (*io).io_references;
    /* io_unlock(io) — no-op */
    if refs == 0 {
        let otype = ((*io).io_bits & IO_BITS_OTYPE) >> 16;
        kmem_cache_free(
            addr_of_mut!(crate::ipc_object::ipc_object_caches[otype as usize]),
            io as vm_offset_t,
        );
    }
}

// ---------------------------------------------------------------------------
//  Port locks (delegate to the object inside the port's ipc_target)
// ---------------------------------------------------------------------------

#[inline]
pub(crate) unsafe fn ip_lock(_p: *mut ipc_port) {}
#[inline]
pub(crate) unsafe fn ip_unlock(_p: *mut ipc_port) {}
#[inline]
pub(crate) unsafe fn ip_lock_init(p: *mut ipc_port) {
    io_lock_init(addr_of_mut!((*p).ip_target.ipt_object));
}
#[inline]
pub(crate) unsafe fn ip_lock_try(_p: *mut ipc_port) -> bool { true }

#[inline]
pub(crate) unsafe fn ip_active(p: *mut ipc_port) -> bool {
    io_active(addr_of_mut!((*p).ip_target.ipt_object))
}

#[inline]
pub(crate) unsafe fn ip_reference(p: *mut ipc_port) {
    io_reference(addr_of_mut!((*p).ip_target.ipt_object));
}

#[inline]
pub(crate) unsafe fn ip_release(p: *mut ipc_port) {
    io_release(addr_of_mut!((*p).ip_target.ipt_object));
}

#[inline]
pub(crate) unsafe fn ip_check_unlock(p: *mut ipc_port) {
    io_check_unlock(addr_of_mut!((*p).ip_target.ipt_object));
}

// ---------------------------------------------------------------------------
//  Port-set locks
// ---------------------------------------------------------------------------

#[inline]
pub(crate) unsafe fn ips_lock(_ps: ipc_pset_t) {}
#[inline]
pub(crate) unsafe fn ips_unlock(_ps: ipc_pset_t) {}

#[inline]
pub(crate) unsafe fn ips_active(ps: ipc_pset_t) -> bool {
    io_active(addr_of_mut!((*ps).ips_target.ipt_object))
}

// ---------------------------------------------------------------------------
//  Message queue locks (NCPUS == 1: no-ops)
// ---------------------------------------------------------------------------

#[inline]
pub(crate) unsafe fn imq_lock(_mq: *mut ipc_mqueue) {}
#[inline]
pub(crate) unsafe fn imq_unlock(_mq: *mut ipc_mqueue) {}
#[inline]
pub(crate) unsafe fn imq_lock_init(_mq: *mut ipc_mqueue) {}
#[inline]
pub(crate) unsafe fn imq_lock_try(_mq: *mut ipc_mqueue) -> bool { true }

// ---------------------------------------------------------------------------
//  Space read-write locks — REAL even at NCPUS == 1.
// ---------------------------------------------------------------------------

#[inline]
pub(crate) unsafe fn is_read_lock(space: ipc_space_t) {
    lock_read(addr_of_mut!((*space).is_lock_data));
}
#[inline]
pub(crate) unsafe fn is_read_unlock(space: ipc_space_t) {
    lock_done(addr_of_mut!((*space).is_lock_data));
}
#[inline]
pub(crate) unsafe fn is_write_lock(space: ipc_space_t) {
    lock_write(addr_of_mut!((*space).is_lock_data));
}
#[inline]
pub(crate) unsafe fn is_write_unlock(space: ipc_space_t) {
    lock_done(addr_of_mut!((*space).is_lock_data));
}
