//! Port of `ipc/ipc_right.c`.
//!
//! Functions to manipulate IPC capabilities.

use core::ptr::{addr_of, addr_of_mut, null_mut};

use crate::extern_c::{
    kmem_cache_free, lock_done, lock_write, rdxtree_insert_common,
    rdxtree_lookup_common, rdxtree_remove,
};
use crate::ipc_entry::ipc_entry_cache;
use crate::ipc_marequest::{ipc_marequest_cancel, ipc_marequest_rename};
use crate::ipc_notify::{
    ipc_notify_dead_name, ipc_notify_no_senders, ipc_notify_port_deleted,
    ipc_notify_send_once,
};
use crate::ipc_object::ipc_object_caches;
use crate::ipc_port::{
    ipc_port_dncancel, ipc_port_dngrow, ipc_port_dnrequest,
    ipc_port_clear_receiver, ipc_port_destroy,
};
use crate::ipc_pset::ipc_pset_destroy;
use crate::mach_types::{
    boolean_t, ipc_entry, ipc_entry_t, ipc_object, ipc_object_bits_t,
    ipc_port, ipc_port_request_index_t, ipc_port_request_t, ipc_port_t,
    ipc_pset_t, ipc_space_t, kern_return_t, mach_msg_type_name_t,
    mach_port_delta_t, mach_port_mscount_t, mach_port_name_t,
    mach_port_right_t, mach_port_type_t, mach_port_urefs_t, rdxtree_key_t,
    vm_offset_t, IE_BITS_MAREQUEST, IE_BITS_RIGHT_MASK, IE_BITS_TYPE_MASK,
    IE_BITS_UREFS_MASK, IE_NULL, IO_BITS_OTYPE, IO_DEAD, IPS_NULL, IS_NULL,
    IS_FREE_LIST_SIZE_LIMIT, KERN_INVALID_ARGUMENT, KERN_INVALID_NAME,
    KERN_INVALID_RIGHT, KERN_INVALID_TASK, KERN_INVALID_VALUE,
    KERN_SUCCESS, KERN_UREFS_OVERFLOW, MACH_MSG_TYPE_COPY_SEND,
    MACH_MSG_TYPE_MAKE_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE,
    MACH_MSG_TYPE_MOVE_RECEIVE, MACH_MSG_TYPE_MOVE_SEND,
    MACH_MSG_TYPE_MOVE_SEND_ONCE, MACH_MSG_TYPE_PORT_RECEIVE,
    MACH_MSG_TYPE_PORT_SEND, MACH_MSG_TYPE_PORT_SEND_ONCE,
    MACH_PORT_NAME_NULL, MACH_PORT_RIGHT_DEAD_NAME, MACH_PORT_RIGHT_NUMBER,
    MACH_PORT_RIGHT_PORT_SET, MACH_PORT_RIGHT_RECEIVE,
    MACH_PORT_RIGHT_SEND, MACH_PORT_RIGHT_SEND_ONCE,
    MACH_PORT_TYPE_DEAD_NAME, MACH_PORT_TYPE_DNREQUEST,
    MACH_PORT_TYPE_MAREQUEST, MACH_PORT_TYPE_NONE, MACH_PORT_TYPE_PORT_RIGHTS,
    MACH_PORT_TYPE_PORT_SET, MACH_PORT_TYPE_PORT_OR_DEAD,
    MACH_PORT_TYPE_RECEIVE, MACH_PORT_TYPE_SEND, MACH_PORT_TYPE_SEND_ONCE,
    MACH_PORT_TYPE_SEND_RECEIVE, MACH_PORT_TYPE_SEND_RIGHTS,
    MACH_PORT_UREFS_MAX, VM_MIN_KERNEL_ADDRESS,
};

// ---------------------------------------------------------------------------
//  Inline expansions of the lock / refcount macros (NCPUS == 1: lock no-ops).
// ---------------------------------------------------------------------------

#[inline]
unsafe fn io_active(io: *mut ipc_object) -> bool {
    ((*io).io_bits as i32) < 0
}
#[inline]
unsafe fn io_otype(io: *mut ipc_object) -> u32 {
    ((*io).io_bits & IO_BITS_OTYPE) >> 16
}

#[inline]
unsafe fn io_lock(_io: *mut ipc_object) {}
#[inline]
unsafe fn io_unlock(_io: *mut ipc_object) {}
#[inline]
unsafe fn ip_lock(_port: ipc_port_t) {}
#[inline]
unsafe fn ip_unlock(_port: ipc_port_t) {}
#[inline]
unsafe fn ips_lock(_pset: ipc_pset_t) {}
#[inline]
unsafe fn ips_unlock(_pset: ipc_pset_t) {}

#[inline]
unsafe fn ip_active(port: ipc_port_t) -> bool {
    io_active(addr_of_mut!((*port).ip_target.ipt_object))
}
#[inline]
unsafe fn ips_active(pset: ipc_pset_t) -> bool {
    io_active(addr_of_mut!((*pset).ips_target.ipt_object))
}
#[inline]
unsafe fn ip_reference(port: ipc_port_t) {
    (*port).ip_target.ipt_object.io_references += 1;
}
#[inline]
unsafe fn ip_release(port: ipc_port_t) {
    (*port).ip_target.ipt_object.io_references -= 1;
}
#[inline]
unsafe fn ip_check_unlock(port: ipc_port_t) {
    let io = addr_of_mut!((*port).ip_target.ipt_object);
    let refs = (*io).io_references;
    if refs == 0 {
        let otype = io_otype(io);
        kmem_cache_free(
            addr_of_mut!(ipc_object_caches[otype as usize]),
            io as vm_offset_t,
        );
    }
}

#[inline]
unsafe fn is_write_lock(space: ipc_space_t) {
    lock_write(addr_of_mut!((*space).is_lock_data));
}
#[inline]
unsafe fn is_write_unlock(space: ipc_space_t) {
    lock_done(addr_of_mut!((*space).is_lock_data));
}

#[inline]
unsafe fn ipc_port_release(port: ipc_port_t) {
    crate::ipc_object::ipc_object_release(
        addr_of_mut!((*port).ip_target.ipt_object),
    );
}

#[inline]
unsafe fn ipc_port_flag_protected_payload_clear(port: ipc_port_t) {
    (*port).ip_target.ipt_object.io_bits &=
        !crate::mach_types::IO_BITS_PROTECTED_PAYLOAD;
}

#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}
#[inline]
fn ie_bits_urefs(bits: u32) -> u32 {
    bits & IE_BITS_UREFS_MASK
}

#[inline]
fn mach_port_urefs_overflow(urefs: u32, delta: i32) -> bool {
    delta > 0
        && (urefs.wrapping_add(delta as u32) <= urefs
            || urefs.wrapping_add(delta as u32) > MACH_PORT_UREFS_MAX)
}
#[inline]
fn mach_port_urefs_underflow(urefs: u32, delta: i32) -> bool {
    delta < 0 && (-delta as u32) > urefs
}

/// Mirror of the static inline `ipc_entry_lookup` from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_lookup(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> ipc_entry_t {
    let entry = rdxtree_lookup_common(
        addr_of!((*space).is_map),
        name as rdxtree_key_t,
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

/// `ipc_entry_dealloc` — static inline from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_dealloc(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) {
    if ((*space).is_free_list_size as usize) < IS_FREE_LIST_SIZE_LIMIT {
        (*space).is_free_list_size += 1;
        /* IE_BITS_GEN_MASK == 0 */
        (*entry).ie_bits = 0;
        (*entry).index.next_free = (*space).is_free_list;
        (*space).is_free_list = entry;
    } else {
        rdxtree_remove(
            addr_of_mut!((*space).is_map),
            name as rdxtree_key_t,
        );
        kmem_cache_free(addr_of_mut!(ipc_entry_cache), entry as vm_offset_t);
    }
    (*space).is_size -= 1;
}

/// `KEY(X)` — `(((unsigned long) X - VM_MIN_KERNEL_ADDRESS) >> 3)` cast to
/// the rdxtree key type.
#[inline]
fn key_for(obj: *mut ipc_object) -> rdxtree_key_t {
    (((obj as u32).wrapping_sub(VM_MIN_KERNEL_ADDRESS)) >> 3) as rdxtree_key_t
}

#[inline]
unsafe fn ipc_reverse_insert(
    space: ipc_space_t,
    obj: *mut ipc_object,
    entry: ipc_entry_t,
) -> kern_return_t {
    rdxtree_insert_common(
        addr_of_mut!((*space).is_reverse_map),
        key_for(obj),
        entry as *mut core::ffi::c_void,
        null_mut(),
    ) as kern_return_t
}

#[inline]
unsafe fn ipc_reverse_remove(
    space: ipc_space_t,
    obj: *mut ipc_object,
) -> ipc_entry_t {
    rdxtree_remove(
        addr_of_mut!((*space).is_reverse_map),
        key_for(obj),
    ) as ipc_entry_t
}

#[inline]
unsafe fn ipc_reverse_lookup(
    space: ipc_space_t,
    obj: *mut ipc_object,
) -> ipc_entry_t {
    rdxtree_lookup_common(
        addr_of!((*space).is_reverse_map),
        key_for(obj),
        0,
    ) as ipc_entry_t
}

/// `ipc_right_dncancel_macro` — return IP_NULL early if no request slot,
/// else delegate to `ipc_right_dncancel`.
#[inline]
unsafe fn ipc_right_dncancel_macro(
    space: ipc_space_t,
    port: ipc_port_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) -> ipc_port_t {
    if (*entry).index.request == 0 {
        null_mut()
    } else {
        ipc_right_dncancel(space, port, name, entry)
    }
}

/// `ipc_port_dnrename(port, index, oname, nname)` macro from ipc/ipc_port.h.
#[inline]
unsafe fn ipc_port_dnrename(
    port: ipc_port_t,
    index: ipc_port_request_index_t,
    _oname: mach_port_name_t,
    nname: mach_port_name_t,
) {
    crate::kassert!(ip_active(port), "ip_active(port)");
    let table: ipc_port_request_t = (*port).ip_dnrequests;
    crate::kassert!(!table.is_null(), "table != IPR_NULL");
    let ipr = table.add(index as usize);
    crate::kassert!((*ipr).name.name == _oname, "ipr->ipr_name == oname");
    (*ipr).name.name = nname;
}

// ---------------------------------------------------------------------------
//  ipc_right_lookup_write
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_lookup_write(
    space: ipc_space_t,
    name: mach_port_name_t,
    entryp: *mut ipc_entry_t,
) -> kern_return_t {
    crate::kassert!(space != IS_NULL, "space != IS_NULL");

    is_write_lock(space);

    if (*space).is_active == 0 {
        is_write_unlock(space);
        return KERN_INVALID_TASK;
    }

    let entry = ipc_entry_lookup(space, name);
    if entry == IE_NULL {
        is_write_unlock(space);
        return KERN_INVALID_NAME;
    }

    *entryp = entry;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_right_reverse
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_reverse(
    space: ipc_space_t,
    object: *mut ipc_object,
    namep: *mut mach_port_name_t,
    entryp: *mut ipc_entry_t,
) -> boolean_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");
    /* io_otype assertion skipped — no Rust accessor */

    let port = object as ipc_port_t;

    ip_lock(port);
    if !ip_active(port) {
        ip_unlock(port);
        return 0; /* FALSE */
    }

    if (*port).data.receiver == space {
        let name = (*port).ip_target.ipt_name;
        crate::kassert!(name != MACH_PORT_NAME_NULL, "name != MACH_PORT_NULL");

        let entry = ipc_entry_lookup(space, name);
        crate::kassert!(entry != IE_NULL, "entry != IE_NULL");
        crate::kassert!(((*entry).ie_bits & MACH_PORT_TYPE_RECEIVE) != 0, "entry->ie_bits & MACH_PORT_TYPE_RECEIVE");
        crate::kassert!(port == (*entry).ie_object as ipc_port_t, "port == entry->ie_object");

        *namep = name;
        *entryp = entry;
        return 1; /* TRUE */
    }

    let rev = ipc_reverse_lookup(space, port as *mut ipc_object);
    if !rev.is_null() {
        *entryp = rev;
        *namep = (*rev).ie_name;
        return 1; /* TRUE */
    }

    ip_unlock(port);
    0 /* FALSE */
}

// ---------------------------------------------------------------------------
//  ipc_right_dnrequest
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_dnrequest(
    space: ipc_space_t,
    name: mach_port_name_t,
    immediate: boolean_t,
    notify: ipc_port_t,
    previousp: *mut ipc_port_t,
) -> kern_return_t {
    let mut previous: ipc_port_t = null_mut();

    loop {
        let mut entry: ipc_entry_t = IE_NULL;

        let kr = ipc_right_lookup_write(space, name, &mut entry);
        if kr != KERN_SUCCESS {
            return kr;
        }
        /* space is write-locked and active */

        let mut bits = (*entry).ie_bits;
        if (bits & MACH_PORT_TYPE_PORT_RIGHTS) != 0 {
            let port: ipc_port_t = (*entry).ie_object as ipc_port_t;
            crate::kassert!(!port.is_null(), "port != IP_NULL");

            if ipc_right_check(space, port, name, entry) == 0 {
                /* port is locked and active */

                if notify.is_null() {
                    previous = ipc_right_dncancel_macro(space, port, name, entry);
                    ip_unlock(port);
                    is_write_unlock(space);
                    break;
                }

                /*
                 *  If a registered soright exists, want to atomically
                 *  switch with it.
                 */
                previous = ipc_right_dncancel_macro(space, port, name, entry);

                let mut request: ipc_port_request_index_t = 0;
                let kr2 = ipc_port_dnrequest(port, name, notify, &mut request);
                if kr2 != KERN_SUCCESS {
                    crate::kassert!(previous.is_null(), "previous == IP_NULL");
                    is_write_unlock(space);

                    let kr3 = ipc_port_dngrow(port);
                    /* port is unlocked */
                    if kr3 != KERN_SUCCESS {
                        return kr3;
                    }

                    continue;
                }

                crate::kassert!(request != 0, "request != 0");
                ip_unlock(port);

                (*entry).index.request = request;
                is_write_unlock(space);
                break;
            }

            bits = (*entry).ie_bits;
            crate::kassert!((bits & MACH_PORT_TYPE_DEAD_NAME) != 0, "bits & MACH_PORT_TYPE_DEAD_NAME");
        }

        if (bits & MACH_PORT_TYPE_DEAD_NAME) != 0
            && immediate != 0
            && !notify.is_null()
        {
            let urefs = ie_bits_urefs(bits);
            crate::kassert!(ie_bits_type(bits) == MACH_PORT_TYPE_DEAD_NAME, "IE_BITS_TYPE(bits) == MACH_PORT_TYPE_DEAD_NAME");
            crate::kassert!(urefs > 0, "urefs > 0");

            if mach_port_urefs_overflow(urefs, 1) {
                is_write_unlock(space);
                return KERN_UREFS_OVERFLOW;
            }

            (*entry).ie_bits = bits + 1; /* increment urefs */
            is_write_unlock(space);

            ipc_notify_dead_name(notify, name);
            previous = null_mut();
            break;
        }

        is_write_unlock(space);
        if (bits & MACH_PORT_TYPE_PORT_OR_DEAD) != 0 {
            return KERN_INVALID_ARGUMENT;
        } else {
            return KERN_INVALID_RIGHT;
        }
    }

    *previousp = previous;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_right_dncancel
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_dncancel(
    _space: ipc_space_t,
    port: ipc_port_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) -> ipc_port_t {
    crate::kassert!(ip_active(port), "ip_active(port)");
    crate::kassert!(port == (*entry).ie_object as ipc_port_t, "port == entry->ie_object");

    let dnrequest = ipc_port_dncancel(port, name, (*entry).index.request);
    (*entry).index.request = 0;
    dnrequest
}

// ---------------------------------------------------------------------------
//  ipc_right_inuse
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_inuse(
    space: ipc_space_t,
    _name: mach_port_name_t,
    entry: ipc_entry_t,
) -> boolean_t {
    let bits = (*entry).ie_bits;

    if ie_bits_type(bits) != MACH_PORT_TYPE_NONE {
        is_write_unlock(space);
        return 1; /* TRUE */
    }
    0 /* FALSE */
}

// ---------------------------------------------------------------------------
//  ipc_right_check
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_check(
    space: ipc_space_t,
    port: ipc_port_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) -> boolean_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");
    crate::kassert!(port == (*entry).ie_object as ipc_port_t, "port == entry->ie_object");

    ip_lock(port);
    if ip_active(port) {
        return 0; /* FALSE */
    }
    ip_unlock(port);

    /* this was either a pure send right or a send-once right */
    let mut bits = (*entry).ie_bits;
    crate::kassert!((bits & MACH_PORT_TYPE_RECEIVE) == 0, "(bits & MACH_PORT_TYPE_RECEIVE) == 0");
    crate::kassert!(ie_bits_urefs(bits) > 0, "IE_BITS_UREFS(bits) > 0");

    if (bits & MACH_PORT_TYPE_SEND) != 0 {
        /* clean up msg-accepted request */
        if (bits & IE_BITS_MAREQUEST) != 0 {
            bits &= !IE_BITS_MAREQUEST;
            ipc_marequest_cancel(space, name);
        }
        ipc_reverse_remove(space, port as *mut ipc_object);
    }

    ipc_port_release(port);

    /* convert entry to dead name */
    bits = (bits & !IE_BITS_TYPE_MASK) | MACH_PORT_TYPE_DEAD_NAME;

    if (*entry).index.request != 0 {
        crate::kassert!(ie_bits_urefs(bits) < MACH_PORT_UREFS_MAX, "IE_BITS_UREFS(bits) < MACH_PORT_UREFS_MAX");
        (*entry).index.request = 0;
        bits += 1; /* increment urefs */
    }

    (*entry).ie_bits = bits;
    (*entry).ie_object = null_mut();

    1 /* TRUE */
}

// ---------------------------------------------------------------------------
//  ipc_right_clean
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_clean(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) {
    let bits = (*entry).ie_bits;
    let typ = ie_bits_type(bits);
    crate::kassert!((*space).is_active == 0, "!space->is_active");

    if typ == MACH_PORT_TYPE_DEAD_NAME {
        /* nothing to do */
    } else if typ == MACH_PORT_TYPE_PORT_SET {
        let pset = (*entry).ie_object as ipc_pset_t;
        crate::kassert!(pset != IPS_NULL, "pset != IPS_NULL");

        ips_lock(pset);
        crate::kassert!(ips_active(pset), "ips_active(pset)");
        ipc_pset_destroy(pset); /* consumes ref, unlocks */
    } else if typ == MACH_PORT_TYPE_SEND
        || typ == MACH_PORT_TYPE_RECEIVE
        || typ == MACH_PORT_TYPE_SEND_RECEIVE
        || typ == MACH_PORT_TYPE_SEND_ONCE
    {
        let port = (*entry).ie_object as ipc_port_t;
        let mut nsrequest: ipc_port_t = null_mut();
        let mut mscount: mach_port_mscount_t = 0;

        crate::kassert!(!port.is_null(), "port != IP_NULL");
        ip_lock(port);

        if !ip_active(port) {
            ip_release(port);
            ip_check_unlock(port);
            return;
        }

        let dnrequest = ipc_right_dncancel_macro(space, port, name, entry);

        if (typ & MACH_PORT_TYPE_SEND) != 0 {
            crate::kassert!((*port).ip_srights > 0, "port->ip_srights > 0");
            (*port).ip_srights -= 1;
            if (*port).ip_srights == 0 {
                nsrequest = (*port).ip_nsrequest;
                if !nsrequest.is_null() {
                    (*port).ip_nsrequest = null_mut();
                    mscount = (*port).ip_mscount;
                }
            }
        }

        if (typ & MACH_PORT_TYPE_RECEIVE) != 0 {
            crate::kassert!((*port).ip_target.ipt_name == name, "port->ip_receiver_name == name");
            crate::kassert!((*port).data.receiver == space, "port->ip_receiver == space");
            ipc_port_clear_receiver(port);
            ipc_port_destroy(port); /* consumes our ref, unlocks */
        } else if (typ & MACH_PORT_TYPE_SEND_ONCE) != 0 {
            crate::kassert!((*port).ip_sorights > 0, "port->ip_sorights > 0");
            ip_unlock(port);
            ipc_notify_send_once(port);
        } else {
            crate::kassert!((*port).data.receiver != space, "port->ip_receiver != space");
            ip_release(port);
            ip_unlock(port); /* port is active */
        }

        if !nsrequest.is_null() {
            ipc_notify_no_senders(nsrequest, mscount);
        }
        if !dnrequest.is_null() {
            ipc_notify_port_deleted(dnrequest, name);
        }
    } else {
        crate::kpanic!("ipc_right_clean: strange type");
    }
}

// ---------------------------------------------------------------------------
//  ipc_right_destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_destroy(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) -> kern_return_t {
    let bits = (*entry).ie_bits;
    let typ = ie_bits_type(bits);
    crate::kassert!((*space).is_active != 0, "space->is_active");

    if typ == MACH_PORT_TYPE_DEAD_NAME {
        ipc_entry_dealloc(space, name, entry);
        is_write_unlock(space);
    } else if typ == MACH_PORT_TYPE_PORT_SET {
        let pset = (*entry).ie_object as ipc_pset_t;
        crate::kassert!(pset != IPS_NULL, "pset != IPS_NULL");

        (*entry).ie_object = null_mut();
        ipc_entry_dealloc(space, name, entry);

        ips_lock(pset);
        crate::kassert!(ips_active(pset), "ips_active(pset)");
        is_write_unlock(space);

        ipc_pset_destroy(pset); /* consumes ref, unlocks */
    } else if typ == MACH_PORT_TYPE_SEND
        || typ == MACH_PORT_TYPE_RECEIVE
        || typ == MACH_PORT_TYPE_SEND_RECEIVE
        || typ == MACH_PORT_TYPE_SEND_ONCE
    {
        let port = (*entry).ie_object as ipc_port_t;
        let mut nsrequest: ipc_port_t = null_mut();
        let mut mscount: mach_port_mscount_t = 0;
        crate::kassert!(!port.is_null(), "port != IP_NULL");

        if (bits & IE_BITS_MAREQUEST) != 0 {
            ipc_marequest_cancel(space, name);
        }

        if typ == MACH_PORT_TYPE_SEND {
            ipc_reverse_remove(space, port as *mut ipc_object);
        }

        ip_lock(port);

        if !ip_active(port) {
            ip_release(port);
            ip_check_unlock(port);

            (*entry).index.request = 0;
            (*entry).ie_object = null_mut();
            ipc_entry_dealloc(space, name, entry);
            is_write_unlock(space);
            return KERN_SUCCESS;
        }

        let dnrequest = ipc_right_dncancel_macro(space, port, name, entry);

        (*entry).ie_object = null_mut();
        ipc_entry_dealloc(space, name, entry);
        is_write_unlock(space);

        if (typ & MACH_PORT_TYPE_SEND) != 0 {
            crate::kassert!((*port).ip_srights > 0, "port->ip_srights > 0");
            (*port).ip_srights -= 1;
            if (*port).ip_srights == 0 {
                nsrequest = (*port).ip_nsrequest;
                if !nsrequest.is_null() {
                    (*port).ip_nsrequest = null_mut();
                    mscount = (*port).ip_mscount;
                }
            }
        }

        if (typ & MACH_PORT_TYPE_RECEIVE) != 0 {
            ipc_port_clear_receiver(port);
            ipc_port_destroy(port);
        } else if (typ & MACH_PORT_TYPE_SEND_ONCE) != 0 {
            ip_unlock(port);
            ipc_notify_send_once(port);
        } else {
            ip_release(port);
            ip_unlock(port);
        }

        if !nsrequest.is_null() {
            ipc_notify_no_senders(nsrequest, mscount);
        }
        if !dnrequest.is_null() {
            ipc_notify_port_deleted(dnrequest, name);
        }
    } else {
        crate::kpanic!("ipc_right_destroy: strange type");
    }

    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_right_dealloc
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_dealloc(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) -> kern_return_t {
    let mut bits = (*entry).ie_bits;
    let mut typ = ie_bits_type(bits);
    crate::kassert!((*space).is_active != 0, "space->is_active");

    'dispatch: loop {
        if typ == MACH_PORT_TYPE_DEAD_NAME {
            /* dead_name path: */
            if ie_bits_urefs(bits) == 1 {
                ipc_entry_dealloc(space, name, entry);
            } else {
                (*entry).ie_bits = bits - 1; /* decrement urefs */
            }
            is_write_unlock(space);
            return KERN_SUCCESS;
        } else if typ == MACH_PORT_TYPE_SEND_ONCE {
            let port = (*entry).ie_object as ipc_port_t;
            crate::kassert!(!port.is_null(), "port != IP_NULL");

            if ipc_right_check(space, port, name, entry) != 0 {
                bits = (*entry).ie_bits;
                typ = MACH_PORT_TYPE_DEAD_NAME; /* goto dead_name */
                continue 'dispatch;
            }
            /* port is locked and active */

            let dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
            ip_unlock(port);

            (*entry).ie_object = null_mut();
            ipc_entry_dealloc(space, name, entry);
            is_write_unlock(space);

            ipc_notify_send_once(port);

            if !dnrequest.is_null() {
                ipc_notify_port_deleted(dnrequest, name);
            }
            return KERN_SUCCESS;
        } else if typ == MACH_PORT_TYPE_SEND {
            let port = (*entry).ie_object as ipc_port_t;
            let mut dnrequest: ipc_port_t = null_mut();
            let mut nsrequest: ipc_port_t = null_mut();
            let mut mscount: mach_port_mscount_t = 0;

            /* assert(port != IP_NULL); */
            if ipc_right_check(space, port, name, entry) != 0 {
                bits = (*entry).ie_bits;
                typ = MACH_PORT_TYPE_DEAD_NAME;
                continue 'dispatch;
            }
            /* port is locked and active */

            if ie_bits_urefs(bits) == 1 {
                (*port).ip_srights -= 1;
                if (*port).ip_srights == 0 {
                    nsrequest = (*port).ip_nsrequest;
                    if !nsrequest.is_null() {
                        (*port).ip_nsrequest = null_mut();
                        mscount = (*port).ip_mscount;
                    }
                }

                dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
                ipc_reverse_remove(space, port as *mut ipc_object);

                if (bits & IE_BITS_MAREQUEST) != 0 {
                    ipc_marequest_cancel(space, name);
                }

                ip_release(port);
                (*entry).ie_object = null_mut();
                ipc_entry_dealloc(space, name, entry);
            } else {
                (*entry).ie_bits = bits - 1; /* decrement urefs */
            }

            ip_unlock(port);
            is_write_unlock(space);

            if !nsrequest.is_null() {
                ipc_notify_no_senders(nsrequest, mscount);
            }
            if !dnrequest.is_null() {
                ipc_notify_port_deleted(dnrequest, name);
            }
            return KERN_SUCCESS;
        } else if typ == MACH_PORT_TYPE_SEND_RECEIVE {
            let port = (*entry).ie_object as ipc_port_t;
            let mut nsrequest: ipc_port_t = null_mut();
            let mut mscount: mach_port_mscount_t = 0;

            crate::kassert!(!port.is_null(), "port != IP_NULL");
            ip_lock(port);

            if ie_bits_urefs(bits) == 1 {
                (*port).ip_srights -= 1;
                if (*port).ip_srights == 0 {
                    nsrequest = (*port).ip_nsrequest;
                    if !nsrequest.is_null() {
                        (*port).ip_nsrequest = null_mut();
                        mscount = (*port).ip_mscount;
                    }
                }
                (*entry).ie_bits =
                    bits & !(IE_BITS_UREFS_MASK | MACH_PORT_TYPE_SEND);
            } else {
                (*entry).ie_bits = bits - 1; /* decrement urefs */
            }

            ip_unlock(port);
            is_write_unlock(space);

            if !nsrequest.is_null() {
                ipc_notify_no_senders(nsrequest, mscount);
            }
            return KERN_SUCCESS;
        } else {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }
    }
}

// ---------------------------------------------------------------------------
//  ipc_right_delta
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_delta(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
    right: mach_port_right_t,
    delta: mach_port_delta_t,
) -> kern_return_t {
    let mut bits = (*entry).ie_bits;
    crate::kassert!((*space).is_active != 0, "space->is_active");
    crate::kassert!(right < MACH_PORT_RIGHT_NUMBER, "right < MACH_PORT_RIGHT_NUMBER");

    if right == MACH_PORT_RIGHT_PORT_SET {
        if (bits & MACH_PORT_TYPE_PORT_SET) == 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }
        if delta == 0 {
            is_write_unlock(space);
            return KERN_SUCCESS;
        }
        if delta != -1 {
            is_write_unlock(space);
            return KERN_INVALID_VALUE;
        }

        let pset = (*entry).ie_object as ipc_pset_t;
        crate::kassert!(pset != IPS_NULL, "pset != IPS_NULL");

        (*entry).ie_object = null_mut();
        ipc_entry_dealloc(space, name, entry);

        ips_lock(pset);
        crate::kassert!(ips_active(pset), "ips_active(pset)");
        is_write_unlock(space);

        ipc_pset_destroy(pset); /* consumes ref, unlocks */
        return KERN_SUCCESS;
    } else if right == MACH_PORT_RIGHT_RECEIVE {
        let mut dnrequest: ipc_port_t = null_mut();

        if (bits & MACH_PORT_TYPE_RECEIVE) == 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }
        if delta == 0 {
            is_write_unlock(space);
            return KERN_SUCCESS;
        }
        if delta != -1 {
            is_write_unlock(space);
            return KERN_INVALID_VALUE;
        }

        if (bits & IE_BITS_MAREQUEST) != 0 {
            bits &= !IE_BITS_MAREQUEST;
            ipc_marequest_cancel(space, name);
        }

        let port = (*entry).ie_object as ipc_port_t;
        crate::kassert!(!port.is_null(), "port != IP_NULL");

        ip_lock(port);
        crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!((*port).ip_target.ipt_name == name, "port->ip_receiver_name == name");
        crate::kassert!((*port).data.receiver == space, "port->ip_receiver == space");

        if (bits & MACH_PORT_TYPE_SEND) != 0 {
            /*
             *  The remaining send right turns into a dead name.
             */
            bits &= !IE_BITS_TYPE_MASK;
            bits |= MACH_PORT_TYPE_DEAD_NAME;

            if (*entry).index.request != 0 {
                (*entry).index.request = 0;
                bits += 1; /* increment urefs */
            }

            (*entry).ie_bits = bits;
            (*entry).ie_object = null_mut();
        } else {
            dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
            (*entry).ie_object = null_mut();
            ipc_entry_dealloc(space, name, entry);
        }
        is_write_unlock(space);

        ipc_port_clear_receiver(port);
        ipc_port_destroy(port);

        if !dnrequest.is_null() {
            ipc_notify_port_deleted(dnrequest, name);
        }
        return KERN_SUCCESS;
    } else if right == MACH_PORT_RIGHT_SEND_ONCE {
        if (bits & MACH_PORT_TYPE_SEND_ONCE) == 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }

        if delta > 0 || delta < -1 {
            is_write_unlock(space);
            return KERN_INVALID_VALUE;
        }

        let port = (*entry).ie_object as ipc_port_t;
        crate::kassert!(!port.is_null(), "port != IP_NULL");

        if ipc_right_check(space, port, name, entry) != 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }
        /* port is locked and active */

        if delta == 0 {
            ip_unlock(port);
            is_write_unlock(space);
            return KERN_SUCCESS;
        }

        let dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
        ip_unlock(port);

        (*entry).ie_object = null_mut();
        ipc_entry_dealloc(space, name, entry);
        is_write_unlock(space);

        ipc_notify_send_once(port);

        if !dnrequest.is_null() {
            ipc_notify_port_deleted(dnrequest, name);
        }
        return KERN_SUCCESS;
    } else if right == MACH_PORT_RIGHT_DEAD_NAME {
        if (bits & MACH_PORT_TYPE_SEND_RIGHTS) != 0 {
            let port = (*entry).ie_object as ipc_port_t;
            crate::kassert!(!port.is_null(), "port != IP_NULL");

            if ipc_right_check(space, port, name, entry) == 0 {
                /* port is locked and active */
                ip_unlock(port);
                is_write_unlock(space);
                return KERN_INVALID_RIGHT;
            }
            bits = (*entry).ie_bits;
        } else if (bits & MACH_PORT_TYPE_DEAD_NAME) == 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }

        let urefs = ie_bits_urefs(bits);
        if mach_port_urefs_underflow(urefs, delta) {
            is_write_unlock(space);
            return KERN_INVALID_VALUE;
        }
        if mach_port_urefs_overflow(urefs, delta) {
            is_write_unlock(space);
            return KERN_UREFS_OVERFLOW;
        }

        if urefs.wrapping_add(delta as u32) == 0 {
            ipc_entry_dealloc(space, name, entry);
        } else {
            (*entry).ie_bits = bits.wrapping_add(delta as u32);
        }

        is_write_unlock(space);
        return KERN_SUCCESS;
    } else if right == MACH_PORT_RIGHT_SEND {
        if (bits & MACH_PORT_TYPE_SEND) == 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }

        let urefs = ie_bits_urefs(bits);
        if mach_port_urefs_underflow(urefs, delta) {
            is_write_unlock(space);
            return KERN_INVALID_VALUE;
        }
        if mach_port_urefs_overflow(urefs.wrapping_add(1), delta) {
            is_write_unlock(space);
            return KERN_UREFS_OVERFLOW;
        }

        let port = (*entry).ie_object as ipc_port_t;
        let mut dnrequest: ipc_port_t = null_mut();
        let mut nsrequest: ipc_port_t = null_mut();
        let mut mscount: mach_port_mscount_t = 0;

        crate::kassert!(!port.is_null(), "port != IP_NULL");

        if ipc_right_check(space, port, name, entry) != 0 {
            is_write_unlock(space);
            return KERN_INVALID_RIGHT;
        }
        /* port is locked and active */

        if urefs.wrapping_add(delta as u32) == 0 {
            (*port).ip_srights -= 1;
            if (*port).ip_srights == 0 {
                nsrequest = (*port).ip_nsrequest;
                if !nsrequest.is_null() {
                    (*port).ip_nsrequest = null_mut();
                    mscount = (*port).ip_mscount;
                }
            }

            if (bits & MACH_PORT_TYPE_RECEIVE) != 0 {
                (*entry).ie_bits =
                    bits & !(IE_BITS_UREFS_MASK | MACH_PORT_TYPE_SEND);
            } else {
                dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
                ipc_reverse_remove(space, port as *mut ipc_object);
                if (bits & IE_BITS_MAREQUEST) != 0 {
                    ipc_marequest_cancel(space, name);
                }
                ip_release(port);
                (*entry).ie_object = null_mut();
                ipc_entry_dealloc(space, name, entry);
            }
        } else {
            (*entry).ie_bits = bits.wrapping_add(delta as u32);
        }

        ip_unlock(port);
        is_write_unlock(space);

        if !nsrequest.is_null() {
            ipc_notify_no_senders(nsrequest, mscount);
        }
        if !dnrequest.is_null() {
            ipc_notify_port_deleted(dnrequest, name);
        }
        return KERN_SUCCESS;
    }

crate::kpanic!("ipc_right_delta: strange right");
    is_write_unlock(space);
    KERN_INVALID_RIGHT
}

// ---------------------------------------------------------------------------
//  ipc_right_info
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_info(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
    typep: *mut mach_port_type_t,
    urefsp: *mut mach_port_urefs_t,
) -> kern_return_t {
    let mut bits = (*entry).ie_bits;

    if (bits & MACH_PORT_TYPE_SEND_RIGHTS) != 0 {
        let port = (*entry).ie_object as ipc_port_t;

        if ipc_right_check(space, port, name, entry) != 0 {
            bits = (*entry).ie_bits;
        } else {
            ip_unlock(port);
        }
    }

    let mut typ = ie_bits_type(bits);
    let request = (*entry).index.request;

    if request != 0 {
        typ |= MACH_PORT_TYPE_DNREQUEST;
    }
    if (bits & IE_BITS_MAREQUEST) != 0 {
        typ |= MACH_PORT_TYPE_MAREQUEST;
    }

    *typep = typ;
    *urefsp = ie_bits_urefs(bits);
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_right_copyin_check
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_copyin_check(
    _space: ipc_space_t,
    _name: mach_port_name_t,
    entry: ipc_entry_t,
    msgt_name: mach_msg_type_name_t,
) -> boolean_t {
    let bits = (*entry).ie_bits;

    if msgt_name == MACH_MSG_TYPE_MAKE_SEND
        || msgt_name == MACH_MSG_TYPE_MAKE_SEND_ONCE
        || msgt_name == MACH_MSG_TYPE_MOVE_RECEIVE
    {
        if (bits & MACH_PORT_TYPE_RECEIVE) == 0 {
            return 0; /* FALSE */
        }
    } else if msgt_name == MACH_MSG_TYPE_COPY_SEND
        || msgt_name == MACH_MSG_TYPE_MOVE_SEND
        || msgt_name == MACH_MSG_TYPE_MOVE_SEND_ONCE
    {
        if (bits & MACH_PORT_TYPE_DEAD_NAME) != 0 {
            return 1;
        }
        if (bits & MACH_PORT_TYPE_SEND_RIGHTS) == 0 {
            return 0; /* FALSE */
        }

        let port = (*entry).ie_object as ipc_port_t;
        ip_lock(port);
        let active = ip_active(port);
        ip_unlock(port);

        if !active {
            return 1;
        }

        if msgt_name == MACH_MSG_TYPE_MOVE_SEND_ONCE {
            if (bits & MACH_PORT_TYPE_SEND_ONCE) == 0 {
                return 0;
            }
        } else if (bits & MACH_PORT_TYPE_SEND) == 0 {
            return 0;
        }
    } else {
        crate::kpanic!("ipc_right_copyin_check: strange rights");
    }

    1 /* TRUE */
}

// ---------------------------------------------------------------------------
//  ipc_right_copyin
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_copyin(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
    msgt_name: mach_msg_type_name_t,
    deadok: boolean_t,
    objectp: *mut *mut ipc_object,
    sorightp: *mut ipc_port_t,
) -> kern_return_t {
    let mut bits = (*entry).ie_bits;
    crate::kassert!((*space).is_active != 0, "space->is_active");

    if msgt_name == MACH_MSG_TYPE_MAKE_SEND {
        if (bits & MACH_PORT_TYPE_RECEIVE) == 0 {
            return KERN_INVALID_RIGHT;
        }
        let port = (*entry).ie_object as ipc_port_t;

        ip_lock(port);
        (*port).ip_mscount += 1;
        (*port).ip_srights += 1;
        ip_reference(port);
        ip_unlock(port);

        *objectp = port as *mut ipc_object;
        *sorightp = null_mut();
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_MAKE_SEND_ONCE {
        if (bits & MACH_PORT_TYPE_RECEIVE) == 0 {
            return KERN_INVALID_RIGHT;
        }
        let port = (*entry).ie_object as ipc_port_t;

        ip_lock(port);
        (*port).ip_sorights += 1;
        ip_reference(port);
        ip_unlock(port);

        *objectp = port as *mut ipc_object;
        *sorightp = null_mut();
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_MOVE_RECEIVE {
        if (bits & MACH_PORT_TYPE_RECEIVE) == 0 {
            return KERN_INVALID_RIGHT;
        }
        let port = (*entry).ie_object as ipc_port_t;
        let mut dnrequest: ipc_port_t = null_mut();

        ip_lock(port);

        if (bits & MACH_PORT_TYPE_SEND) != 0 {
            (*entry).ie_name = name;
            ipc_reverse_insert(space, port as *mut ipc_object, entry);
            ip_reference(port);
        } else {
            dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
            if (bits & IE_BITS_MAREQUEST) != 0 {
                ipc_marequest_cancel(space, name);
            }
            (*entry).ie_object = null_mut();
        }
        (*entry).ie_bits = bits & !MACH_PORT_TYPE_RECEIVE;

        ipc_port_clear_receiver(port);

        (*port).ip_target.ipt_name = MACH_PORT_NAME_NULL;
        (*port).data.destination = null_mut();
        ipc_port_flag_protected_payload_clear(port);
        ip_unlock(port);

        *objectp = port as *mut ipc_object;
        *sorightp = dnrequest;
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_COPY_SEND {
        if (bits & MACH_PORT_TYPE_DEAD_NAME) != 0 {
            return copy_dead_helper(bits, deadok, objectp, sorightp);
        }
        if (bits & MACH_PORT_TYPE_SEND_RIGHTS) == 0 {
            return KERN_INVALID_RIGHT;
        }

        let port = (*entry).ie_object as ipc_port_t;
        if ipc_right_check(space, port, name, entry) != 0 {
            bits = (*entry).ie_bits;
            return copy_dead_helper(bits, deadok, objectp, sorightp);
        }
        /* port is locked and active */

        if (bits & MACH_PORT_TYPE_SEND) == 0 {
            ip_unlock(port);
            return KERN_INVALID_RIGHT;
        }

        (*port).ip_srights += 1;
        ip_reference(port);
        ip_unlock(port);

        *objectp = port as *mut ipc_object;
        *sorightp = null_mut();
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_MOVE_SEND {
        if (bits & MACH_PORT_TYPE_DEAD_NAME) != 0 {
            return move_dead_helper(entry, bits, deadok, objectp, sorightp);
        }
        if (bits & MACH_PORT_TYPE_SEND_RIGHTS) == 0 {
            return KERN_INVALID_RIGHT;
        }

        let port = (*entry).ie_object as ipc_port_t;
        if ipc_right_check(space, port, name, entry) != 0 {
            bits = (*entry).ie_bits;
            return move_dead_helper(entry, bits, deadok, objectp, sorightp);
        }

        if (bits & MACH_PORT_TYPE_SEND) == 0 {
            ip_unlock(port);
            return KERN_INVALID_RIGHT;
        }

        let mut dnrequest: ipc_port_t = null_mut();

        if ie_bits_urefs(bits) == 1 {
            if (bits & MACH_PORT_TYPE_RECEIVE) != 0 {
                ip_reference(port);
            } else {
                dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
                ipc_reverse_remove(space, port as *mut ipc_object);
                if (bits & IE_BITS_MAREQUEST) != 0 {
                    ipc_marequest_cancel(space, name);
                }
                (*entry).ie_object = null_mut();
            }
            (*entry).ie_bits =
                bits & !(IE_BITS_UREFS_MASK | MACH_PORT_TYPE_SEND);
        } else {
            (*port).ip_srights += 1;
            ip_reference(port);
            (*entry).ie_bits = bits - 1; /* decrement urefs */
        }

        ip_unlock(port);

        *objectp = port as *mut ipc_object;
        *sorightp = dnrequest;
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_MOVE_SEND_ONCE {
        if (bits & MACH_PORT_TYPE_DEAD_NAME) != 0 {
            return move_dead_helper(entry, bits, deadok, objectp, sorightp);
        }
        if (bits & MACH_PORT_TYPE_SEND_RIGHTS) == 0 {
            return KERN_INVALID_RIGHT;
        }

        let port = (*entry).ie_object as ipc_port_t;
        if ipc_right_check(space, port, name, entry) != 0 {
            bits = (*entry).ie_bits;
            return move_dead_helper(entry, bits, deadok, objectp, sorightp);
        }

        if (bits & MACH_PORT_TYPE_SEND_ONCE) == 0 {
            ip_unlock(port);
            return KERN_INVALID_RIGHT;
        }

        let dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
        ip_unlock(port);

        (*entry).ie_object = null_mut();
        (*entry).ie_bits = bits & !MACH_PORT_TYPE_SEND_ONCE;

        *objectp = port as *mut ipc_object;
        *sorightp = dnrequest;
        return KERN_SUCCESS;
    }

crate::kpanic!("ipc_right_copyin: strange rights");
}

#[inline]
unsafe fn copy_dead_helper(
    _bits: u32,
    deadok: boolean_t,
    objectp: *mut *mut ipc_object,
    sorightp: *mut ipc_port_t,
) -> kern_return_t {
    if deadok == 0 {
        return KERN_INVALID_RIGHT;
    }
    *objectp = IO_DEAD;
    *sorightp = null_mut();
    KERN_SUCCESS
}

#[inline]
unsafe fn move_dead_helper(
    entry: ipc_entry_t,
    bits: u32,
    deadok: boolean_t,
    objectp: *mut *mut ipc_object,
    sorightp: *mut ipc_port_t,
) -> kern_return_t {
    if deadok == 0 {
        return KERN_INVALID_RIGHT;
    }
    if ie_bits_urefs(bits) == 1 {
        (*entry).ie_bits = bits & !MACH_PORT_TYPE_DEAD_NAME;
    } else {
        (*entry).ie_bits = bits - 1;
    }
    *objectp = IO_DEAD;
    *sorightp = null_mut();
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_right_copyin_undo
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_copyin_undo(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
    _msgt_name: mach_msg_type_name_t,
    object: *mut ipc_object,
    soright: ipc_port_t,
) {
    let bits = (*entry).ie_bits;
    crate::kassert!((*space).is_active != 0, "space->is_active");

    if !soright.is_null() {
        (*entry).ie_bits =
            (bits & !IE_BITS_RIGHT_MASK) | MACH_PORT_TYPE_DEAD_NAME | 2;
    } else if ie_bits_type(bits) == MACH_PORT_TYPE_NONE {
        (*entry).ie_bits =
            (bits & !IE_BITS_RIGHT_MASK) | MACH_PORT_TYPE_DEAD_NAME | 1;
    } else if ie_bits_type(bits) == MACH_PORT_TYPE_DEAD_NAME {
        if _msgt_name != MACH_MSG_TYPE_COPY_SEND {
            (*entry).ie_bits = bits + 1;
        }
    } else {
        if _msgt_name != MACH_MSG_TYPE_COPY_SEND {
            (*entry).ie_bits = bits + 1;
        }

        ipc_right_check(space, object as ipc_port_t, name, entry);
        /* object is dead, so not locked */
    }

    /* release the reference acquired by copyin */
    if object != IO_DEAD {
        crate::ipc_object::ipc_object_release(object);
    }
}

// ---------------------------------------------------------------------------
//  ipc_right_copyin_two
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_copyin_two(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
    objectp: *mut *mut ipc_object,
    sorightp: *mut ipc_port_t,
) -> kern_return_t {
    let bits = (*entry).ie_bits;
    crate::kassert!((*space).is_active != 0, "space->is_active");

    if (bits & MACH_PORT_TYPE_SEND) == 0 {
        return KERN_INVALID_RIGHT;
    }

    let urefs = ie_bits_urefs(bits);
    if urefs < 2 {
        return KERN_INVALID_RIGHT;
    }

    let port = (*entry).ie_object as ipc_port_t;
    crate::kassert!(!port.is_null(), "port != IP_NULL");

    if ipc_right_check(space, port, name, entry) != 0 {
        return KERN_INVALID_RIGHT;
    }
    /* port is locked and active */

    let mut dnrequest: ipc_port_t = null_mut();

    if urefs == 2 {
        if (bits & MACH_PORT_TYPE_RECEIVE) != 0 {
            (*port).ip_srights += 1;
            ip_reference(port);
            ip_reference(port);
        } else {
            dnrequest = ipc_right_dncancel_macro(space, port, name, entry);
            ipc_reverse_remove(space, port as *mut ipc_object);
            if (bits & IE_BITS_MAREQUEST) != 0 {
                ipc_marequest_cancel(space, name);
            }
            (*port).ip_srights += 1;
            ip_reference(port);
            (*entry).ie_object = null_mut();
        }
        (*entry).ie_bits =
            bits & !(IE_BITS_UREFS_MASK | MACH_PORT_TYPE_SEND);
    } else {
        (*port).ip_srights += 2;
        ip_reference(port);
        ip_reference(port);
        (*entry).ie_bits = bits - 2; /* decrement urefs */
    }
    ip_unlock(port);

    *objectp = port as *mut ipc_object;
    *sorightp = dnrequest;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_right_copyout
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_copyout(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
    msgt_name: mach_msg_type_name_t,
    overflow: boolean_t,
    object: *mut ipc_object,
) -> kern_return_t {
    let bits = (*entry).ie_bits;
    crate::kassert!((object as usize) != 0 && (object as usize) != !0usize, "IO_VALID(object)");
    crate::kassert!(io_otype(object) == crate::mach_types::IOT_PORT, "io_otype(object) == IOT_PORT");
    crate::kassert!(io_active(object), "io_active(object)");
    crate::kassert!((*entry).ie_object == object, "entry->ie_object == object");

    let port = object as ipc_port_t;

    if msgt_name == MACH_MSG_TYPE_PORT_SEND_ONCE {
        ip_unlock(port);
        (*entry).ie_bits = bits | (MACH_PORT_TYPE_SEND_ONCE | 1);
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_PORT_SEND {
        if (bits & MACH_PORT_TYPE_SEND) != 0 {
            let urefs = ie_bits_urefs(bits);

            if urefs + 1 == MACH_PORT_UREFS_MAX {
                if overflow != 0 {
                    /* leave urefs pegged to maximum */
                    (*port).ip_srights -= 1;
                    ip_release(port);
                    ip_unlock(port);
                    return KERN_SUCCESS;
                }
                ip_unlock(port);
                return KERN_UREFS_OVERFLOW;
            }

            (*port).ip_srights -= 1;
            ip_release(port);
            ip_unlock(port);
        } else if (bits & MACH_PORT_TYPE_RECEIVE) != 0 {
            /* transfer send right to entry */
            ip_release(port);
            ip_unlock(port);
        } else {
            /* transfer send right and ref to entry */
            ip_unlock(port);

            (*entry).ie_name = name;
            ipc_reverse_insert(space, port as *mut ipc_object, entry);
        }

        (*entry).ie_bits = (bits | MACH_PORT_TYPE_SEND) + 1;
        return KERN_SUCCESS;
    } else if msgt_name == MACH_MSG_TYPE_PORT_RECEIVE {
        let dest = (*port).data.destination;

        (*port).ip_target.ipt_name = name;
        (*port).data.receiver = space;
        ipc_port_flag_protected_payload_clear(port);

        if (bits & MACH_PORT_TYPE_SEND) != 0 {
            ip_release(port);
            ip_unlock(port);
            ipc_reverse_remove(space, port as *mut ipc_object);
        } else {
            /* transfer ref to entry */
            ip_unlock(port);
        }

        (*entry).ie_bits = bits | MACH_PORT_TYPE_RECEIVE;

        if !dest.is_null() {
            ipc_port_release(dest);
        }
        return KERN_SUCCESS;
    }

crate::kpanic!("ipc_right_copyout: strange rights");
}

// ---------------------------------------------------------------------------
//  ipc_right_rename
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_right_rename(
    space: ipc_space_t,
    oname: mach_port_name_t,
    oentry: ipc_entry_t,
    nname: mach_port_name_t,
    nentry: ipc_entry_t,
) -> kern_return_t {
    let mut bits = (*oentry).ie_bits;
    let mut request = (*oentry).index.request;
    let mut object = (*oentry).ie_object;

    crate::kassert!((*space).is_active != 0, "space->is_active");
    crate::kassert!(oname != nname, "oname != nname");

    if request != 0 {
        let port = object as ipc_port_t;
        crate::kassert!((bits & MACH_PORT_TYPE_PORT_RIGHTS) != 0, "bits & MACH_PORT_TYPE_PORT_RIGHTS");
        crate::kassert!(!port.is_null(), "port != IP_NULL");

        if ipc_right_check(space, port, oname, oentry) != 0 {
            bits = (*oentry).ie_bits;
            request = 0;
            object = null_mut();
        } else {
            /* port is locked and active */
            ipc_port_dnrename(port, request, oname, nname);
            ip_unlock(port);
            (*oentry).index.request = 0;
        }
    }

    if (bits & IE_BITS_MAREQUEST) != 0 {
        ipc_marequest_rename(space, oname, nname);
    }

    /* initialize nentry */
    (*nentry).ie_bits |= bits & IE_BITS_RIGHT_MASK;
    (*nentry).index.request = request;
    (*nentry).ie_object = object;

    let typ = ie_bits_type(bits);
    if typ == MACH_PORT_TYPE_SEND {
        let port = object as ipc_port_t;
        crate::kassert!(!port.is_null(), "port != IP_NULL");
        ipc_reverse_remove(space, port as *mut ipc_object);
        (*nentry).ie_name = nname;
        ipc_reverse_insert(space, port as *mut ipc_object, nentry);
    } else if typ == MACH_PORT_TYPE_RECEIVE
        || typ == MACH_PORT_TYPE_SEND_RECEIVE
    {
        let port = object as ipc_port_t;
        crate::kassert!(!port.is_null(), "port != IP_NULL");
        ip_lock(port);
        crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!((*port).ip_target.ipt_name == oname, "port->ip_receiver_name == oname");
        crate::kassert!((*port).data.receiver == space, "port->ip_receiver == space");
        (*port).ip_target.ipt_name = nname;
        ip_unlock(port);
    } else if typ == MACH_PORT_TYPE_PORT_SET {
        let pset = object as ipc_pset_t;
        crate::kassert!(pset != IPS_NULL, "pset != IPS_NULL");
        ips_lock(pset);
        crate::kassert!(ips_active(pset), "ips_active(pset)");
        crate::kassert!((*pset).ips_target.ipt_name == oname, "pset->ips_local_name == oname");
        (*pset).ips_target.ipt_name = nname;
        ips_unlock(pset);
    } else if typ == MACH_PORT_TYPE_SEND_ONCE
        || typ == MACH_PORT_TYPE_DEAD_NAME
    {
        /* nothing extra */
    } else {
        crate::kpanic!("ipc_right_rename: strange rights");
    }

    crate::kassert!((*oentry).index.request == 0, "oentry->ie_request == 0");
    (*oentry).ie_object = null_mut();
    ipc_entry_dealloc(space, oname, oentry);
    is_write_unlock(space);
    KERN_SUCCESS
}
