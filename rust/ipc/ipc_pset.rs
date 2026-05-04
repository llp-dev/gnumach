//! Port of `ipc/ipc_pset.c`.
//!
//! Functions to manipulate IPC port sets.

use core::ptr::addr_of_mut;

use crate::extern_c::{
    kmem_cache_free, lock_done,
};
use crate::ipc_mqueue::{ipc_mqueue_changed, ipc_mqueue_move};
use crate::ipc_object::{
    ipc_object_alloc, ipc_object_alloc_name, ipc_object_caches,
};
use crate::ipc_target::{ipc_target_init, ipc_target_terminate};
use crate::mach_types::{
    ipc_object, ipc_object_bits_t, ipc_port, ipc_port_t, ipc_pset, ipc_pset_t,
    ipc_space_t, kern_return_t, mach_port_name_t, vm_offset_t, IOT_PORT_SET,
    IO_BITS_ACTIVE, IO_BITS_OTYPE, IPS_NULL, KERN_NOT_IN_SET, KERN_SUCCESS,
    MACH_PORT_TYPE_PORT_SET, MACH_RCV_PORT_CHANGED, MACH_RCV_PORT_DIED,
};

// ---------------------------------------------------------------------------
//  Macros expanded inline
// ---------------------------------------------------------------------------

#[inline]
unsafe fn io_active(io: *mut ipc_object) -> bool {
    ((*io).io_bits as i32) < 0
}

#[inline]
unsafe fn ips_active(pset: ipc_pset_t) -> bool {
    io_active(addr_of_mut!((*pset).ips_target.ipt_object))
}

#[inline]
unsafe fn ip_active(port: ipc_port_t) -> bool {
    io_active(addr_of_mut!((*port).ip_target.ipt_object))
}

#[inline]
unsafe fn ips_reference(pset: ipc_pset_t) {
    (*pset).ips_target.ipt_object.io_references += 1;
}

#[inline]
unsafe fn ips_release(pset: ipc_pset_t) {
    (*pset).ips_target.ipt_object.io_references -= 1;
}

/// `io_check_unlock(io)`: read refs, unlock, free if zero.  With NCPUS=1
/// `io_unlock` is a no-op; `io_free(otype, io)` becomes `kmem_cache_free`.
#[inline]
unsafe fn ips_check_unlock(pset: ipc_pset_t) {
    let io: *mut ipc_object = addr_of_mut!((*pset).ips_target.ipt_object);
    let refs = (*io).io_references;
    /* io_unlock(io) — simple_unlock no-op with NCPUS == 1 */
    if refs == 0 {
        let otype: ipc_object_bits_t = ((*io).io_bits & IO_BITS_OTYPE) >> 16;
        let cache = addr_of_mut!(ipc_object_caches[otype as usize]);
        kmem_cache_free(cache, io as vm_offset_t);
    }
}

/// All simple_lock / simple_unlock primitives are no-ops with NCPUS == 1.
#[inline]
unsafe fn ip_lock(_port: ipc_port_t) {}
#[inline]
unsafe fn ip_unlock(_port: ipc_port_t) {}
#[inline]
unsafe fn ips_lock(_pset: ipc_pset_t) {}
#[inline]
unsafe fn ips_unlock(_pset: ipc_pset_t) {}
#[inline]
unsafe fn imq_lock(_mq: *mut crate::mach_types::ipc_mqueue) {}
#[inline]
unsafe fn imq_unlock(_mq: *mut crate::mach_types::ipc_mqueue) {}

#[inline]
unsafe fn is_read_unlock(space: ipc_space_t) {
    lock_done(addr_of_mut!((*space).is_lock_data));
}

// ---------------------------------------------------------------------------
//  ipc_pset_alloc
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_pset_alloc(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
    psetp: *mut ipc_pset_t,
) -> kern_return_t {
    let mut pset: ipc_pset_t = IPS_NULL;
    let mut name: mach_port_name_t = 0;

    let kr = ipc_object_alloc(
        space,
        IOT_PORT_SET,
        MACH_PORT_TYPE_PORT_SET,
        0,
        &mut name,
        &mut pset as *mut ipc_pset_t as *mut *mut ipc_object,
    );
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* pset is locked */

    ipc_target_init(addr_of_mut!((*pset).ips_target), name);

    *namep = name;
    *psetp = pset;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_pset_alloc_name
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_pset_alloc_name(
    space: ipc_space_t,
    name: mach_port_name_t,
    psetp: *mut ipc_pset_t,
) -> kern_return_t {
    let mut pset: ipc_pset_t = IPS_NULL;

    let kr = ipc_object_alloc_name(
        space,
        IOT_PORT_SET,
        MACH_PORT_TYPE_PORT_SET,
        0,
        name,
        &mut pset as *mut ipc_pset_t as *mut *mut ipc_object,
    );
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* pset is locked */

    ipc_target_init(addr_of_mut!((*pset).ips_target), name);

    *psetp = pset;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_pset_add
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_pset_add(pset: ipc_pset_t, port: ipc_port_t) {
    crate::kassert!(ips_active(pset), "ips_active(pset)");
    crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!((*port).ip_pset == IPS_NULL, "port->ip_pset == IPS_NULL");

    (*port).ip_pset = pset;
    (*port).ip_cur_target = addr_of_mut!((*pset).ips_target);
    ips_reference(pset);

    let port_mq = addr_of_mut!((*port).ip_target.ipt_messages);
    let pset_mq = addr_of_mut!((*pset).ips_target.ipt_messages);

    imq_lock(port_mq);
    imq_lock(pset_mq);

    /* move messages from port's queue to the port set's queue */

    ipc_mqueue_move(pset_mq, port_mq, port);
    imq_unlock(pset_mq);
    crate::kassert!(
        (*port).ip_target.ipt_messages.imq_messages.ikmq_base.is_null(),
        "ipc_kmsg_queue_empty(&port->ip_messages.imq_messages)"
    );

    /* wake up threads waiting to receive from the port */

    ipc_mqueue_changed(port_mq, MACH_RCV_PORT_CHANGED);
    crate::kassert!(
        (*port).ip_target.ipt_messages.imq_threads.ithq_base.is_null(),
        "ipc_thread_queue_empty(&port->ip_messages.imq_threads)"
    );
    imq_unlock(port_mq);
}

// ---------------------------------------------------------------------------
//  ipc_pset_remove
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_pset_remove(pset: ipc_pset_t, port: ipc_port_t) {
    crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!((*port).ip_pset == pset, "port->ip_pset == pset");

    (*port).ip_pset = IPS_NULL;
    (*port).ip_cur_target = addr_of_mut!((*port).ip_target);
    ips_release(pset);

    let port_mq = addr_of_mut!((*port).ip_target.ipt_messages);
    let pset_mq = addr_of_mut!((*pset).ips_target.ipt_messages);

    imq_lock(port_mq);
    imq_lock(pset_mq);

    /* move messages from port set's queue to the port's queue */

    ipc_mqueue_move(port_mq, pset_mq, port);

    imq_unlock(pset_mq);
    imq_unlock(port_mq);
}

// ---------------------------------------------------------------------------
//  ipc_pset_move
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_pset_move(
    space: ipc_space_t,
    port: ipc_port_t,
    nset: ipc_pset_t,
) -> kern_return_t {
    let mut oset: ipc_pset_t;

    /*
     *  While we've got the space locked, it holds refs for
     *  the port and nset (because of the entries).  Also,
     *  they must be alive.  While we've got port locked, it
     *  holds a ref for oset, which might not be alive.
     */

    ip_lock(port);
    crate::kassert!(ip_active(port), "ip_active(port)");

    oset = (*port).ip_pset;

    if oset == nset {
        /* the port is already in the new set:  a noop */

        is_read_unlock(space);
    } else if oset == IPS_NULL {
        /* just add port to the new set */

        ips_lock(nset);
        crate::kassert!(ips_active(nset), "ips_active(nset)");
        is_read_unlock(space);

        ipc_pset_add(nset, port);

        ips_unlock(nset);
    } else if nset == IPS_NULL {
        /* just remove port from the old set */

        is_read_unlock(space);
        ips_lock(oset);

        ipc_pset_remove(oset, port);

        if ips_active(oset) {
            ips_unlock(oset);
        } else {
            ips_check_unlock(oset);
            oset = IPS_NULL; /* trigger KERN_NOT_IN_SET */
        }
    } else {
        /* atomically move port from oset to nset */

        if (oset as usize) < (nset as usize) {
            ips_lock(oset);
            ips_lock(nset);
        } else {
            ips_lock(nset);
            ips_lock(oset);
        }

        is_read_unlock(space);
        crate::kassert!(ips_active(nset), "ips_active(nset)");

        ipc_pset_remove(oset, port);
        ipc_pset_add(nset, port);

        ips_unlock(nset);
        ips_check_unlock(oset); /* KERN_NOT_IN_SET not a possibility */
    }

    ip_unlock(port);

    if (nset == IPS_NULL) && (oset == IPS_NULL) {
        KERN_NOT_IN_SET
    } else {
        KERN_SUCCESS
    }
}

// ---------------------------------------------------------------------------
//  ipc_pset_destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_pset_destroy(pset: ipc_pset_t) {
    crate::kassert!(ips_active(pset), "ips_active(pset)");

    (*pset).ips_target.ipt_object.io_bits &= !IO_BITS_ACTIVE;

    let pset_mq = addr_of_mut!((*pset).ips_target.ipt_messages);
    imq_lock(pset_mq);
    ipc_mqueue_changed(pset_mq, MACH_RCV_PORT_DIED);
    imq_unlock(pset_mq);

    /* Common destruction for the IPC target.  */
    ipc_target_terminate(addr_of_mut!((*pset).ips_target));

    ips_release(pset); /* consume the ref our caller gave us */
    ips_check_unlock(pset);
}
