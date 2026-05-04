//! Port of `ipc/ipc_port.c`.
//!
//! Functions to manipulate IPC ports.

use core::mem::size_of;
use core::ptr::{addr_of_mut, null_mut};

use crate::extern_c::{
    ipc_kobject_destroy, kmem_cache_alloc, rdxtree_lookup_common, thread_go,
};
use crate::ipc_kmsg::{ipc_kmsg_destroy, ipc_kmsg_dequeue};
use crate::ipc_mqueue::{ipc_mqueue_changed, ipc_mqueue_init};
use crate::ipc_notify::{
    ipc_notify_dead_name, ipc_notify_no_senders, ipc_notify_port_destroyed,
    ipc_notify_send_once,
};
use crate::ipc_object::{ipc_object_caches, ipc_object_copyout};
use crate::ipc_target::{ipc_target_init, ipc_target_terminate};
use crate::mach_types::{
    ipc_entry_t, ipc_object, ipc_object_bits_t, ipc_port, ipc_port_request,
    ipc_port_request_index_t, ipc_port_request_t, ipc_port_t,
    ipc_port_timestamp_t, ipc_pset_t, ipc_space_t, ipc_table_size_t,
    ipc_thread_t, kern_return_t, mach_port_mscount_t,
    mach_port_msgcount_t, mach_port_name_t, mach_port_seqno_t, vm_offset_t,
    IE_BITS_TYPE_MASK, IE_NULL, IKM_NULL, IKOT_NONE, IOT_PORT, IO_BITS_ACTIVE,
    IO_BITS_KOTYPE, IO_BITS_PROTECTED_PAYLOAD, IPS_NULL,
    ITH_NULL, IS_NULL, KERN_NO_SPACE, KERN_RESOURCE_SHORTAGE, KERN_SUCCESS,
    KERN_INVALID_CAPABILITY, MACH_MSG_SUCCESS, MACH_MSG_TYPE_PORT_SEND,
    MACH_PORT_NAME_DEAD, MACH_PORT_NAME_NULL, MACH_PORT_QLIMIT_DEFAULT,
    MACH_PORT_TYPE_RECEIVE, MACH_RCV_PORT_DIED,
};

// ---------------------------------------------------------------------------
//  Globals (defined here, as in C `ipc/ipc_port.c`)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static mut ipc_port_timestamp_data: ipc_port_timestamp_t = 0;

// `def_simple_lock_data(, ipc_port_multiple_lock_data)` and
// `def_simple_lock_data(, ipc_port_timestamp_lock_data)` expand to zero-sized
// struct globals on NCPUS == 1.  No remaining C consumer references these
// symbols (the C ipc/ipc_init.c that called the *_lock_init macros is now
// in Rust), so the symbols themselves do not need to be emitted.

// ---------------------------------------------------------------------------
//  Macros expanded inline (1:1 with the C originals)
// ---------------------------------------------------------------------------

use crate::locks::{
    io_check_unlock, ip_active, ip_check_unlock, ip_lock, ip_lock_init,
    ip_lock_try, ip_reference, ip_release, ip_unlock,
};

#[inline]
unsafe fn io_kotype(io: *mut ipc_object) -> u32 {
    (*io).io_bits & IO_BITS_KOTYPE
}

#[inline]
fn io_makebits(active: bool, otype: u32, kotype: u32) -> ipc_object_bits_t {
    let active_bit = if active { IO_BITS_ACTIVE } else { 0 };
    active_bit | (otype << 16) | kotype
}

#[inline]
unsafe fn io_alloc(otype: u32) -> *mut ipc_object {
    kmem_cache_alloc(addr_of_mut!(ipc_object_caches[otype as usize]))
        as *mut ipc_object
}
#[inline]
unsafe fn ip_kotype(port: *mut ipc_port) -> u32 {
    io_kotype(addr_of_mut!((*port).ip_target.ipt_object))
}
#[inline]
unsafe fn ip_alloc() -> ipc_port_t {
    io_alloc(IOT_PORT) as ipc_port_t
}

/// `ipc_port_release(port)` macro (= ipc_object_release on the port's object).
#[inline]
unsafe fn ipc_port_release(port: ipc_port_t) {
    crate::ipc_object::ipc_object_release(addr_of_mut!((*port).ip_target.ipt_object));
}

/// `ipc_port_set_mscount(port, mscount)`.
#[inline]
unsafe fn ipc_port_set_mscount(port: ipc_port_t, mscount: mach_port_mscount_t) {
    crate::kassert!(ip_active(port), "ip_active(port)");
    (*port).ip_mscount = mscount;
}

#[inline]
unsafe fn ipc_port_flag_protected_payload_set(port: ipc_port_t) {
    (*port).ip_target.ipt_object.io_bits |= IO_BITS_PROTECTED_PAYLOAD;
}

#[inline]
unsafe fn ipc_port_flag_protected_payload_clear(port: ipc_port_t) {
    (*port).ip_target.ipt_object.io_bits &= !IO_BITS_PROTECTED_PAYLOAD;
}

/// `ipc_thread_queue_init(queue)` — `(queue)->ithq_base = ITH_NULL`.
#[inline]
unsafe fn ipc_thread_queue_init(q: *mut crate::mach_types::ipc_thread_queue) {
    (*q).ithq_base = ITH_NULL;
}

/// All simple_lock primitives no-ops with NCPUS == 1.
#[inline]
unsafe fn ipc_port_multiple_lock() {}
#[inline]
unsafe fn ipc_port_multiple_unlock() {}
#[inline]
unsafe fn ipc_port_timestamp_lock() {}
#[inline]
unsafe fn ipc_port_timestamp_unlock() {}
use crate::locks::{
    imq_lock, imq_unlock, ips_active, ips_lock, ips_unlock,
};

#[inline]
unsafe fn ips_check_unlock(pset: ipc_pset_t) {
    io_check_unlock(addr_of_mut!((*pset).ips_target.ipt_object));
}

/// `ipc_table_alloc(size)` — Rust ipc_table.rs.
#[inline]
unsafe fn ipc_table_alloc(size: crate::mach_types::vm_size_t) -> vm_offset_t {
    crate::ipc_table::ipc_table_alloc(size)
}
#[inline]
unsafe fn ipc_table_free(size: crate::mach_types::vm_size_t, table: vm_offset_t) {
    crate::ipc_table::ipc_table_free(size, table);
}

/// `it_dnrequests_alloc(its)` — kalloc a port-request table sized for `its`.
#[inline]
unsafe fn it_dnrequests_alloc(its: ipc_table_size_t) -> ipc_port_request_t {
    let bytes = (*its).its_size as crate::mach_types::vm_size_t
        * size_of::<ipc_port_request>() as crate::mach_types::vm_size_t;
    ipc_table_alloc(bytes) as ipc_port_request_t
}

/// `it_dnrequests_free(its, table)`.
#[inline]
unsafe fn it_dnrequests_free(its: ipc_table_size_t, table: ipc_port_request_t) {
    let bytes = (*its).its_size as crate::mach_types::vm_size_t
        * size_of::<ipc_port_request>() as crate::mach_types::vm_size_t;
    ipc_table_free(bytes, table as vm_offset_t);
}

/// Mirror of the static inline `ipc_entry_lookup` from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_lookup(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> ipc_entry_t {
    let entry = rdxtree_lookup_common(
        core::ptr::addr_of!((*space).is_map),
        name as crate::mach_types::rdxtree_key_t,
        0,
    ) as ipc_entry_t;
    if entry == IE_NULL {
        return IE_NULL;
    }
    if (*entry).ie_bits & IE_BITS_TYPE_MASK == 0 {
        return IE_NULL;
    }
    entry
}

/// `invalid_port_to_name` from `ipc/port.h` — for the IP_DEAD/IP_NULL case.
/// `mach_port_t` (kernel) is `struct ipc_port *`.  We compare the raw
/// pointer value against MACH_PORT_NULL (0) and MACH_PORT_DEAD (~0).
#[inline]
fn invalid_port_to_name(port: ipc_port_t) -> mach_port_name_t {
    let p = port as usize as u32;
    if p == 0 {
        MACH_PORT_NAME_NULL
    } else if p == !0u32 {
        MACH_PORT_NAME_DEAD
    } else {
        crate::kpanic!("invalid_port_to_name() called with a valid name");
    }
}

// ---------------------------------------------------------------------------
//  ipc_port_timestamp
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_timestamp() -> ipc_port_timestamp_t {
    ipc_port_timestamp_lock();
    let timestamp = ipc_port_timestamp_data;
    ipc_port_timestamp_data = ipc_port_timestamp_data.wrapping_add(1);
    ipc_port_timestamp_unlock();
    timestamp
}

// ---------------------------------------------------------------------------
//  ipc_port_dnrequest
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_dnrequest(
    port: ipc_port_t,
    name: mach_port_name_t,
    soright: ipc_port_t,
    indexp: *mut ipc_port_request_index_t,
) -> kern_return_t {
crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!(name != MACH_PORT_NAME_NULL, "name != MACH_PORT_NULL");
    crate::kassert!(!soright.is_null(), "soright != IP_NULL");

    let table: ipc_port_request_t = (*port).ip_dnrequests;
    if table.is_null() {
        return KERN_NO_SPACE;
    }

    let index = (*table).notify.index;
    if index == 0 {
        return KERN_NO_SPACE;
    }

    let ipr: ipc_port_request_t = table.add(index as usize);
crate::kassert!((*ipr).name.name == MACH_PORT_NAME_NULL, "ipr->ipr_name == MACH_PORT_NULL");

    (*table).notify.index = (*ipr).notify.index;
    (*ipr).name.name = name;
    (*ipr).notify.port = soright;

    *indexp = index;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_port_dngrow
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_dngrow(port: ipc_port_t) -> kern_return_t {
    crate::kassert!(ip_active(port), "ip_active(port)");

    let otable: ipc_port_request_t = (*port).ip_dnrequests;
    let its: ipc_table_size_t = if otable.is_null() {
        crate::ipc_table::ipc_table_dnrequests
    } else {
        (*otable).name.size.add(1)
    };

    ip_reference(port);
    ip_unlock(port);

    let ntable: ipc_port_request_t;
    if (*its).its_size == 0 {
        ipc_port_release(port);
        return KERN_RESOURCE_SHORTAGE;
    }
    ntable = it_dnrequests_alloc(its);
    if ntable.is_null() {
        ipc_port_release(port);
        return KERN_RESOURCE_SHORTAGE;
    }

    ip_lock(port);
    ip_release(port);

    /*
     *  Check that port is still active and that nobody else has slipped in
     *  and grown the table on us.  Note that just checking
     *  port->ip_dnrequests == otable isn't sufficient; must check ipr_size.
     */

    let still_ours = ip_active(port)
        && (*port).ip_dnrequests == otable
        && (otable.is_null()
            || (*otable).name.size.add(1) == its);

    if still_ours {
        let oits: ipc_table_size_t;
        let osize: u32;
        let nsize: u32;
        let mut free: ipc_port_request_index_t;

        /* copy old table to new table */

        if !otable.is_null() {
            oits = (*otable).name.size;
            osize = (*oits).its_size;
            free = (*otable).notify.index;

            core::ptr::copy_nonoverlapping(
                otable.add(1) as *const u8,
                ntable.add(1) as *mut u8,
                ((osize - 1) as usize) * size_of::<ipc_port_request>(),
            );
        } else {
            oits = null_mut();
            osize = 1;
            free = 0;
        }

        nsize = (*its).its_size;
        crate::kassert!(nsize > osize, "nsize > osize");

        /* add new elements to the new table's free list */
        let mut i = osize;
        while i < nsize {
            let ipr: ipc_port_request_t = ntable.add(i as usize);
            (*ipr).name.name = MACH_PORT_NAME_NULL;
            (*ipr).notify.index = free;
            free = i;
            i += 1;
        }

        (*ntable).notify.index = free;
        (*ntable).name.size = its;
        (*port).ip_dnrequests = ntable;
        ip_unlock(port);

        if !otable.is_null() {
            it_dnrequests_free(oits, otable);
        }
    } else {
        ip_check_unlock(port);
        it_dnrequests_free(its, ntable);
    }

    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_port_dncancel
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_dncancel(
    port: ipc_port_t,
    _name: mach_port_name_t,
    index: ipc_port_request_index_t,
) -> ipc_port_t {
crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!(_name != MACH_PORT_NAME_NULL, "name != MACH_PORT_NULL");
    crate::kassert!(index != 0, "index != 0");

    let table: ipc_port_request_t = (*port).ip_dnrequests;
crate::kassert!(!table.is_null(), "table != IPR_NULL");

    let ipr: ipc_port_request_t = table.add(index as usize);
    let dnrequest = (*ipr).notify.port;
crate::kassert!((*ipr).name.name == _name, "ipr->ipr_name == name");

    /* return ipr to the free list inside the table */
    (*ipr).name.name = MACH_PORT_NAME_NULL;
    (*ipr).notify.index = (*table).notify.index;
    (*table).notify.index = index;

    dnrequest
}

// ---------------------------------------------------------------------------
//  ipc_port_pdrequest
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_pdrequest(
    port: ipc_port_t,
    notify: ipc_port_t,
    previousp: *mut ipc_port_t,
) {
    crate::kassert!(ip_active(port), "ip_active(port)");

    let previous = (*port).ip_pdrequest;
    (*port).ip_pdrequest = notify;
    ip_unlock(port);

    *previousp = previous;
}

// ---------------------------------------------------------------------------
//  ipc_port_nsrequest
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_nsrequest(
    port: ipc_port_t,
    sync: mach_port_mscount_t,
    notify: ipc_port_t,
    previousp: *mut ipc_port_t,
) {
    crate::kassert!(ip_active(port), "ip_active(port)");

    let previous = (*port).ip_nsrequest;
    let mscount = (*port).ip_mscount;

    if (*port).ip_srights == 0 && sync <= mscount && !notify.is_null() {
        (*port).ip_nsrequest = null_mut();
        ip_unlock(port);
        ipc_notify_no_senders(notify, mscount);
    } else {
        (*port).ip_nsrequest = notify;
        ip_unlock(port);
    }

    *previousp = previous;
}

// ---------------------------------------------------------------------------
//  ipc_port_set_qlimit
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_set_qlimit(
    port: ipc_port_t,
    qlimit: mach_port_msgcount_t,
) {
    crate::kassert!(ip_active(port), "ip_active(port)");

    if qlimit > (*port).ip_qlimit {
        let wakeup: mach_port_msgcount_t = qlimit - (*port).ip_qlimit;

        let mut i: mach_port_msgcount_t = 0;
        while i < wakeup {
            let th: ipc_thread_t = crate::ipc_thread::ipc_thread_dequeue(
                addr_of_mut!((*port).ip_blocked),
            );
            if th == ITH_NULL {
                break;
            }

            (*th).ith_state = MACH_MSG_SUCCESS;
            thread_go(th);
            i += 1;
        }
    }

    (*port).ip_qlimit = qlimit;
}

// ---------------------------------------------------------------------------
//  ipc_port_lock_mqueue
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_lock_mqueue(
    port: ipc_port_t,
) -> *mut crate::mach_types::ipc_mqueue {
    if (*port).ip_pset != IPS_NULL {
        let pset = (*port).ip_pset;

        ips_lock(pset);
        if ips_active(pset) {
            imq_lock(addr_of_mut!((*pset).ips_target.ipt_messages));
            ips_unlock(pset);
            return addr_of_mut!((*pset).ips_target.ipt_messages);
        }

        crate::ipc_pset::ipc_pset_remove(pset, port);
        ips_check_unlock(pset);
    }

    imq_lock(addr_of_mut!((*port).ip_target.ipt_messages));
    addr_of_mut!((*port).ip_target.ipt_messages)
}

// ---------------------------------------------------------------------------
//  ipc_port_set_seqno
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_set_seqno(
    port: ipc_port_t,
    seqno: mach_port_seqno_t,
) {
    let mqueue = ipc_port_lock_mqueue(port);
    (*port).ip_seqno = seqno;
    imq_unlock(mqueue);
}

// ---------------------------------------------------------------------------
//  ipc_port_set_protected_payload / ipc_port_clear_protected_payload
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_set_protected_payload(
    port: ipc_port_t,
    payload: u32, /* rpc_uintptr_t */
) {
    let mqueue = ipc_port_lock_mqueue(port);
    (*port).ip_protected_payload = payload;
    ipc_port_flag_protected_payload_set(port);
    imq_unlock(mqueue);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_clear_protected_payload(port: ipc_port_t) {
    let mqueue = ipc_port_lock_mqueue(port);
    ipc_port_flag_protected_payload_clear(port);
    imq_unlock(mqueue);
}

// ---------------------------------------------------------------------------
//  ipc_port_clear_receiver
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_clear_receiver(port: ipc_port_t) {
    crate::kassert!(ip_active(port), "ip_active(port)");

    let pset = (*port).ip_pset;
    if pset != IPS_NULL {
        ips_lock(pset);
        crate::ipc_pset::ipc_pset_remove(pset, port);
        ips_check_unlock(pset);
    } else {
        imq_lock(addr_of_mut!((*port).ip_target.ipt_messages));
        ipc_mqueue_changed(
            addr_of_mut!((*port).ip_target.ipt_messages),
            MACH_RCV_PORT_DIED,
        );
        imq_unlock(addr_of_mut!((*port).ip_target.ipt_messages));
    }

    ipc_port_set_mscount(port, 0);
    imq_lock(addr_of_mut!((*port).ip_target.ipt_messages));
    (*port).ip_seqno = 0;
    imq_unlock(addr_of_mut!((*port).ip_target.ipt_messages));
}

// ---------------------------------------------------------------------------
//  ipc_port_init
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_init(
    port: ipc_port_t,
    space: ipc_space_t,
    name: mach_port_name_t,
) {
    /* port->ip_kobject doesn't have to be initialized */

    ipc_target_init(addr_of_mut!((*port).ip_target), name);

    (*port).data.receiver = space;

    (*port).ip_mscount = 0;
    (*port).ip_srights = 0;
    (*port).ip_sorights = 0;

    (*port).ip_nsrequest = null_mut();
    (*port).ip_pdrequest = null_mut();
    (*port).ip_dnrequests = null_mut();

    (*port).ip_pset = IPS_NULL;
    (*port).ip_cur_target = addr_of_mut!((*port).ip_target);
    (*port).ip_seqno = 0;
    (*port).ip_msgcount = 0;
    (*port).ip_qlimit = MACH_PORT_QLIMIT_DEFAULT;
    ipc_port_flag_protected_payload_clear(port);
    (*port).ip_protected_payload = 0;

    ipc_mqueue_init(addr_of_mut!((*port).ip_target.ipt_messages));
    ipc_thread_queue_init(addr_of_mut!((*port).ip_blocked));
}

// ---------------------------------------------------------------------------
//  ipc_port_alloc / ipc_port_alloc_name
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_alloc(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
    portp: *mut ipc_port_t,
) -> kern_return_t {
    let mut port: ipc_port_t = null_mut();
    let mut name: mach_port_name_t = 0;

    let kr = crate::ipc_object::ipc_object_alloc(
        space,
        IOT_PORT,
        MACH_PORT_TYPE_RECEIVE,
        0,
        &mut name,
        &mut port as *mut ipc_port_t as *mut *mut ipc_object,
    );
    if kr != KERN_SUCCESS {
        return kr;
    }

    /* port is locked */
    ipc_port_init(port, space, name);

    *namep = name;
    *portp = port;

    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_alloc_name(
    space: ipc_space_t,
    name: mach_port_name_t,
    portp: *mut ipc_port_t,
) -> kern_return_t {
    let mut port: ipc_port_t = null_mut();

    let kr = crate::ipc_object::ipc_object_alloc_name(
        space,
        IOT_PORT,
        MACH_PORT_TYPE_RECEIVE,
        0,
        name,
        &mut port as *mut ipc_port_t as *mut *mut ipc_object,
    );
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* port is locked */

    ipc_port_init(port, space, name);

    *portp = port;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_port_destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_destroy(port: ipc_port_t) {
crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!((*port).ip_pset == IPS_NULL, "port->ip_pset == IPS_NULL");
    crate::kassert!((*port).ip_mscount == 0, "port->ip_mscount == 0");
    crate::kassert!((*port).ip_seqno == 0, "port->ip_seqno == 0");

    /* first check for a backup port */
    let pdrequest = (*port).ip_pdrequest;
    if !pdrequest.is_null() {
        /* we assume the ref for pdrequest */
        (*port).ip_pdrequest = null_mut();

        /* make port be in limbo */
        (*port).ip_target.ipt_name = MACH_PORT_NAME_NULL;
        (*port).data.destination = null_mut();
        ipc_port_flag_protected_payload_clear(port);
        ip_unlock(port);

        if ipc_port_check_circularity(port, pdrequest) == 0 {
            /* consumes our refs for port and pdrequest */
            ipc_notify_port_destroyed(pdrequest, port);
            return;
        } else {
            /* consume pdrequest and destroy port */
            ipc_port_release_sonce(pdrequest);
        }

        ip_lock(port);
crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!((*port).ip_pset == IPS_NULL, "port->ip_pset == IPS_NULL");
        crate::kassert!((*port).ip_mscount == 0, "port->ip_mscount == 0");
        crate::kassert!((*port).ip_seqno == 0, "port->ip_seqno == 0");
        crate::kassert!((*port).ip_pdrequest.is_null(), "port->ip_pdrequest == IP_NULL");
        crate::kassert!((*port).ip_target.ipt_name == MACH_PORT_NAME_NULL, "port->ip_receiver_name == MACH_PORT_NULL");
        crate::kassert!((*port).data.destination.is_null(), "port->ip_destination == IP_NULL");
    }

    /*
     *  rouse all blocked senders
     *
     *  This must be done with the port locked, because ipc_mqueue_send can
     *  play with the ip_blocked queue of a dead port.
     */
    loop {
        let sender = crate::ipc_thread::ipc_thread_dequeue(
            addr_of_mut!((*port).ip_blocked),
        );
        if sender == ITH_NULL {
            break;
        }
        (*sender).ith_state = MACH_MSG_SUCCESS;
        thread_go(sender);
    }

    /* once port is dead, we don't need to keep it locked */

    (*port).ip_target.ipt_object.io_bits &= !IO_BITS_ACTIVE;
    (*port).data.timestamp = ipc_port_timestamp();
    ip_unlock(port);

    /* throw away no-senders request */

    let nsrequest = (*port).ip_nsrequest;
    if !nsrequest.is_null() {
        ipc_notify_send_once(nsrequest); /* consumes ref */
    }

    /* destroy any queued messages */

    let mqueue = addr_of_mut!((*port).ip_target.ipt_messages);
    imq_lock(mqueue);
crate::kassert!((*mqueue).imq_threads.ithq_base.is_null(), "ipc_thread_queue_empty(&mqueue->imq_threads)");
    let kmqueue = addr_of_mut!((*mqueue).imq_messages);

    loop {
        let kmsg = ipc_kmsg_dequeue(kmqueue);
        if kmsg == IKM_NULL {
            break;
        }
        imq_unlock(mqueue);

crate::kassert!((*kmsg).ikm_header.msgh_remote_port == port as usize as u32, "kmsg->ikm_header.msgh_remote_port == (mach_port_t) port");
        ipc_port_release(port);
        (*kmsg).ikm_header.msgh_remote_port = 0; /* MACH_PORT_NULL */
        ipc_kmsg_destroy(kmsg);

        imq_lock(mqueue);
    }

    imq_unlock(mqueue);

    /* generate dead-name notifications */

    let dnrequests: ipc_port_request_t = (*port).ip_dnrequests;
    if !dnrequests.is_null() {
        let its: ipc_table_size_t = (*dnrequests).name.size;
        let size: u32 = (*its).its_size;
        let mut index: ipc_port_request_index_t = 1;

        while index < size {
            let ipr: ipc_port_request_t = dnrequests.add(index as usize);
            let name = (*ipr).name.name;

            if name == MACH_PORT_NAME_NULL {
                index += 1;
                continue;
            }

            let soright = (*ipr).notify.port;
crate::kassert!(!soright.is_null(), "soright != IP_NULL");
            ipc_notify_dead_name(soright, name);
            index += 1;
        }

        it_dnrequests_free(its, dnrequests);
    }

    if ip_kotype(port) != IKOT_NONE {
        ipc_kobject_destroy(port);
    }

    /* Common destruction for the IPC target.  */
    ipc_target_terminate(addr_of_mut!((*port).ip_target));

    ipc_port_release(port); /* consume caller's ref */
}

// ---------------------------------------------------------------------------
//  ipc_port_check_circularity
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_check_circularity(
    port: ipc_port_t,
    dest: ipc_port_t,
) -> crate::mach_types::boolean_t {
crate::kassert!(!port.is_null(), "port != IP_NULL");
    crate::kassert!(!dest.is_null(), "dest != IP_NULL");

    if port == dest {
        return 1; /* TRUE */
    }
    let mut base: ipc_port_t = dest;

    /*
     *  First try a quick check that can run in parallel.  No circularity
     *  if dest is not in transit.
     */
    'not_circular: {
        ip_lock(port);
        if ip_lock_try(dest) {
            if !ip_active(dest)
                || (*dest).ip_target.ipt_name != MACH_PORT_NAME_NULL
                || (*dest).data.destination.is_null()
            {
                break 'not_circular;
            }
            /* dest is in transit; further checking necessary */
            ip_unlock(dest);
        }
        ip_unlock(port);

        ipc_port_multiple_lock(); /* massive serialization */

        /*
         *  Search for the end of the chain (a port not in transit),
         *  acquiring locks along the way.
         */
        loop {
            ip_lock(base);
            if !ip_active(base)
                || (*base).ip_target.ipt_name != MACH_PORT_NAME_NULL
                || (*base).data.destination.is_null()
            {
                break;
            }
            base = (*base).data.destination;
        }

        /* all ports in chain from dest to base, inclusive, are locked */

        if port == base {
            /* circularity detected! */
            ipc_port_multiple_unlock();
            /* port (== base) is in limbo */

            let mut d = dest;
            while !d.is_null() {
                /* dest is in transit or in limbo */
                let next = (*d).data.destination;
                ip_unlock(d);
                d = next;
            }

            return 1; /* TRUE */
        }

        /*
         *  The guarantee:  lock port while the entire chain is locked.
         *  Once port is locked, we can take a reference to dest, add port
         *  to the chain, and unlock everything.
         */
        ip_lock(port);
        ipc_port_multiple_unlock();

        /* fall through to not_circular */
    }

    /* not_circular: port is in limbo */

    ip_reference(dest);
    (*port).data.destination = dest;

    /* now unlock chain */
    let mut p = port;
    while p != base {
        /* p is in transit */
        let next = (*p).data.destination;
        ip_unlock(p);
        p = next;
    }

    /* base is not in transit */
    ip_unlock(base);

    0 /* FALSE */
}

// ---------------------------------------------------------------------------
//  ipc_port_lookup_notify
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_lookup_notify(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> ipc_port_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");

    let entry = ipc_entry_lookup(space, name);
    if entry == IE_NULL {
        return null_mut();
    }

    if (*entry).ie_bits & MACH_PORT_TYPE_RECEIVE == 0 {
        return null_mut();
    }

let port: ipc_port_t = (*entry).ie_object as ipc_port_t;
    crate::kassert!(!port.is_null(), "port != IP_NULL");

ip_lock(port);
    crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!((*port).ip_target.ipt_name == name, "port->ip_receiver_name == name");
    crate::kassert!((*port).data.receiver == space, "port->ip_receiver == space");

    ip_reference(port);
    (*port).ip_sorights += 1;
    ip_unlock(port);

    port
}

// ---------------------------------------------------------------------------
//  ipc_port_make_send / copy_send / copyout_send / release_send
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_make_send(port: ipc_port_t) -> ipc_port_t {
    crate::kassert!((port as usize) != 0 && (port as usize) != !0usize, "IP_VALID(port)");
    ip_lock(port);
    crate::kassert!(ip_active(port), "ip_active(port)");
    (*port).ip_mscount += 1;
    (*port).ip_srights += 1;
    ip_reference(port);
    ip_unlock(port);
    port
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_copy_send(port: ipc_port_t) -> ipc_port_t {
    /* IP_VALID: not NULL and not (mach_port_t)~0 (= MACH_PORT_DEAD).  We
     * cannot test ip_active without locking; pre-test using the pointer
     * value as the C does. */
    if (port as usize) == 0 || (port as usize) == !0usize {
        return port;
    }

ip_lock(port);
    let sright: ipc_port_t = if ip_active(port) {
        crate::kassert!((*port).ip_srights > 0, "port->ip_srights > 0");
        ip_reference(port);
        (*port).ip_srights += 1;
        port
    } else {
        !0usize as ipc_port_t /* IP_DEAD */
    };
    ip_unlock(port);
    sright
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_copyout_send(
    sright: ipc_port_t,
    space: ipc_space_t,
) -> mach_port_name_t {
    if (sright as usize) != 0 && (sright as usize) != !0usize {
        let mut name: mach_port_name_t = 0;
        let kr = ipc_object_copyout(
            space,
            sright as *mut ipc_object,
            MACH_MSG_TYPE_PORT_SEND,
            1, /* TRUE */
            &mut name,
        );
        if kr != KERN_SUCCESS {
            ipc_port_release_send(sright);

            if kr == KERN_INVALID_CAPABILITY {
                MACH_PORT_NAME_DEAD
            } else {
                MACH_PORT_NAME_NULL
            }
        } else {
            name
        }
    } else {
        invalid_port_to_name(sright)
    }
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_release_send(port: ipc_port_t) {
    let mut nsrequest: ipc_port_t = null_mut();
    let mut mscount: mach_port_mscount_t = 0;

    crate::kassert!((port as usize) != 0 && (port as usize) != !0usize, "IP_VALID(port)");

    ip_lock(port);
    ip_release(port);

    if !ip_active(port) {
        ip_check_unlock(port);
        return;
    }

crate::kassert!((*port).ip_srights > 0, "port->ip_srights > 0");
    (*port).ip_srights -= 1;
    if (*port).ip_srights == 0 {
        nsrequest = (*port).ip_nsrequest;
        if !nsrequest.is_null() {
            (*port).ip_nsrequest = null_mut();
            mscount = (*port).ip_mscount;
        }
    }

    ip_unlock(port);

    if !nsrequest.is_null() {
        ipc_notify_no_senders(nsrequest, mscount);
    }
}

// ---------------------------------------------------------------------------
//  ipc_port_make_sonce / release_sonce / release_receive
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_make_sonce(port: ipc_port_t) -> ipc_port_t {
    crate::kassert!((port as usize) != 0 && (port as usize) != !0usize, "IP_VALID(port)");
    ip_lock(port);
    crate::kassert!(ip_active(port), "ip_active(port)");
    (*port).ip_sorights += 1;
    ip_reference(port);
    ip_unlock(port);
    port
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_release_sonce(port: ipc_port_t) {
    crate::kassert!((port as usize) != 0 && (port as usize) != !0usize, "IP_VALID(port)");
    ip_lock(port);
    crate::kassert!((*port).ip_sorights > 0, "port->ip_sorights > 0");

    (*port).ip_sorights -= 1;
    ip_release(port);

    if !ip_active(port) {
        ip_check_unlock(port);
        return;
    }

    ip_unlock(port);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_release_receive(port: ipc_port_t) {
    crate::kassert!((port as usize) != 0 && (port as usize) != !0usize, "IP_VALID(port)");

    ip_lock(port);
    crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!((*port).ip_target.ipt_name == MACH_PORT_NAME_NULL, "port->ip_receiver_name == MACH_PORT_NULL");
    let dest = (*port).data.destination;

    ipc_port_destroy(port); /* consumes ref, unlocks */

    if !dest.is_null() {
        ipc_port_release(dest);
    }
}

// ---------------------------------------------------------------------------
//  ipc_port_alloc_special / ipc_port_dealloc_special
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_port_alloc_special(space: ipc_space_t) -> ipc_port_t {
    let port: ipc_port_t = ip_alloc();
    if port.is_null() {
        return null_mut();
    }

    ip_lock_init(port);
    (*port).ip_target.ipt_object.io_references = 1;
    (*port).ip_target.ipt_object.io_bits = io_makebits(true, IOT_PORT, 0);

    /*
     *  The actual values of ip_receiver_name aren't important, as long as
     *  they are valid (not null/dead).
     */
    ipc_port_init(port, space, port as usize as mach_port_name_t);

    port
}

#[no_mangle]
pub unsafe extern "C" fn ipc_port_dealloc_special(
    port: ipc_port_t,
    _space: ipc_space_t,
) {
ip_lock(port);
    crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!((*port).ip_target.ipt_name != MACH_PORT_NAME_NULL, "port->ip_receiver_name != MACH_PORT_NULL");
    crate::kassert!((*port).data.receiver == _space, "port->ip_receiver == space");

    /* simplify the ipc_space_kernel check in ipc_mqueue_send */
    (*port).ip_target.ipt_name = MACH_PORT_NAME_NULL;
    (*port).data.receiver = IS_NULL;

    ipc_port_clear_receiver(port);
    ipc_port_destroy(port);
}
