//! Port of `ipc/ipc_object.c`.
//!
//! Functions to manipulate IPC objects.

use core::ptr::{addr_of_mut, null_mut, write_bytes};

use crate::extern_c::{kmem_cache_alloc, kmem_cache_free, rdxtree_remove};
use crate::ipc_entry::{ipc_entry_alloc, ipc_entry_alloc_name, ipc_entry_cache};
use crate::ipc_notify::{ipc_notify_no_senders, ipc_notify_port_deleted, ipc_notify_send_once};
use crate::ipc_port::{ipc_port_release_receive, ipc_port_release_send};
use crate::ipc_right::{
    ipc_right_copyin, ipc_right_copyout, ipc_right_inuse, ipc_right_lookup_write, ipc_right_rename,
    ipc_right_reverse,
};
use crate::ipc_space::ipc_space_kernel;
use crate::mach_types::{
    ipc_entry_t, ipc_object, ipc_object_bits_t, ipc_port, ipc_pset, ipc_space_t, kern_return_t,
    kmem_cache, mach_msg_type_name_t, mach_port_mscount_t, mach_port_name_t, mach_port_right_t,
    mach_port_type_t, mach_port_urefs_t, rdxtree_key_t, vm_offset_t, IE_BITS_TYPE_MASK, IE_NULL,
    IOT_NUMBER, IOT_PORT, IOT_PORT_SET, IO_BITS_ACTIVE, IO_BITS_OTYPE, IO_BITS_PROTECTED_PAYLOAD,
    IS_FREE_LIST_SIZE_LIMIT, KERN_INVALID_CAPABILITY, KERN_INVALID_NAME, KERN_INVALID_RIGHT,
    KERN_INVALID_TASK, KERN_NAME_EXISTS, KERN_RESOURCE_SHORTAGE, KERN_RIGHT_EXISTS, KERN_SUCCESS,
    MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE,
    MACH_MSG_TYPE_MOVE_RECEIVE, MACH_MSG_TYPE_MOVE_SEND, MACH_MSG_TYPE_MOVE_SEND_ONCE,
    MACH_MSG_TYPE_PORT_RECEIVE, MACH_MSG_TYPE_PORT_SEND, MACH_MSG_TYPE_PORT_SEND_ONCE,
    MACH_PORT_NAME_NULL, MACH_PORT_TYPE, MACH_PORT_TYPE_ALL_RIGHTS, MACH_PORT_TYPE_DEAD_NAME,
    MACH_PORT_TYPE_NONE, MACH_PORT_TYPE_SEND_RECEIVE, MACH_PORT_UREFS_MAX, SIZE_OF_KMEM_CACHE,
};

// ---------------------------------------------------------------------------
//  Globals (defined here, as in C `ipc/ipc_object.c`)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static mut ipc_object_caches: [kmem_cache; IOT_NUMBER] = [
    kmem_cache {
        _bytes: [0u8; SIZE_OF_KMEM_CACHE],
    },
    kmem_cache {
        _bytes: [0u8; SIZE_OF_KMEM_CACHE],
    },
];

// ---------------------------------------------------------------------------
//  Macros expanded inline
// ---------------------------------------------------------------------------

use crate::locks::{
    io_active, io_check_unlock, io_lock, io_lock_init, io_reference, io_release, io_unlock,
    ip_active, ip_lock, ip_reference, ip_unlock, is_read_unlock, is_write_lock, is_write_unlock,
};

#[inline]
unsafe fn io_otype(io: *mut ipc_object) -> u32 {
    ((*io).io_bits & IO_BITS_OTYPE) >> 16
}

#[inline]
fn io_makebits(active: bool, otype: u32, kotype: u32) -> ipc_object_bits_t {
    let active_bit = if active { IO_BITS_ACTIVE } else { 0 };
    active_bit | (otype << 16) | kotype
}

#[inline]
unsafe fn io_alloc(otype: u32) -> *mut ipc_object {
    kmem_cache_alloc(addr_of_mut!(ipc_object_caches[otype as usize])) as *mut ipc_object
}

#[inline]
unsafe fn io_free(otype: u32, io: *mut ipc_object) {
    kmem_cache_free(
        addr_of_mut!(ipc_object_caches[otype as usize]),
        io as vm_offset_t,
    );
}

#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}

#[inline]
unsafe fn ie_free(e: ipc_entry_t) {
    kmem_cache_free(addr_of_mut!(ipc_entry_cache), e as vm_offset_t);
}

/// `ipc_port_set_mscount(port, mscount)` — assert active, set mscount.
#[inline]
unsafe fn ipc_port_set_mscount(port: *mut ipc_port, mscount: mach_port_mscount_t) {
    crate::kassert!(ip_active(port), "ip_active(port)");
    (*port).ip_mscount = mscount;
}

/// `ipc_port_flag_protected_payload_clear(port)`.
#[inline]
unsafe fn ipc_port_flag_protected_payload_clear(port: *mut ipc_port) {
    (*port).ip_target.ipt_object.io_bits &= !IO_BITS_PROTECTED_PAYLOAD;
}

/// `ipc_entry_dealloc` — static inline from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_dealloc(space: ipc_space_t, name: mach_port_name_t, entry: ipc_entry_t) {
    crate::kassert!((*space).is_active != 0, "space->is_active");
    crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");
    crate::kassert!((*entry).index.request == 0, "entry->ie_request == 0");

    if ((*space).is_free_list_size as usize) < IS_FREE_LIST_SIZE_LIMIT {
        (*space).is_free_list_size += 1;
        /* IE_BITS_GEN_MASK == 0 */
        (*entry).ie_bits &= 0;
        (*entry).index.next_free = (*space).is_free_list;
        (*space).is_free_list = entry;
    } else {
        rdxtree_remove(addr_of_mut!((*space).is_map), name as rdxtree_key_t);
        ie_free(entry);
    }
    (*space).is_size -= 1;
}

/// Mirror of the static inline `ipc_entry_lookup` from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_lookup(space: ipc_space_t, name: mach_port_name_t) -> ipc_entry_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");
    let entry = crate::extern_c::rdxtree_lookup_common(
        core::ptr::addr_of!((*space).is_map),
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

// ---------------------------------------------------------------------------
//  ipc_object_reference / ipc_object_release
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_reference(object: *mut ipc_object) {
    io_lock(object);
    crate::kassert!((*object).io_references > 0, "object->io_references > 0");
    io_reference(object);
    io_unlock(object);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_object_release(object: *mut ipc_object) {
    io_lock(object);
    crate::kassert!((*object).io_references > 0, "object->io_references > 0");
    io_release(object);
    io_check_unlock(object);
}

// ---------------------------------------------------------------------------
//  ipc_object_translate
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_translate(
    space: ipc_space_t,
    name: mach_port_name_t,
    right: mach_port_right_t,
    objectp: *mut *mut ipc_object,
) -> kern_return_t {
    let mut entry: ipc_entry_t = IE_NULL;

    /* `ipc_right_lookup_read` is `#define`d to `ipc_right_lookup_write` in C. */
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* space is read-locked and active */

    if ((*entry).ie_bits & MACH_PORT_TYPE(right)) == 0 {
        is_read_unlock(space);
        return KERN_INVALID_RIGHT;
    }

    let object = (*entry).ie_object;
    crate::kassert!(!object.is_null(), "object != IO_NULL");

    io_lock(object);
    is_read_unlock(space);

    *objectp = object;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_object_alloc_dead / ipc_object_alloc_dead_name
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_alloc_dead(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
) -> kern_return_t {
    let mut entry: ipc_entry_t = IE_NULL;

    is_write_lock(space);
    let kr = ipc_entry_alloc(space, namep, &mut entry);
    if kr != KERN_SUCCESS {
        is_write_unlock(space);
        return kr;
    }

    /* null object, MACH_PORT_TYPE_DEAD_NAME, 1 uref */
    crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");
    (*entry).ie_bits |= MACH_PORT_TYPE_DEAD_NAME | 1;

    is_write_unlock(space);
    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn ipc_object_alloc_dead_name(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> kern_return_t {
    let mut entry: ipc_entry_t = IE_NULL;

    is_write_lock(space);
    let kr = ipc_entry_alloc_name(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        is_write_unlock(space);
        return kr;
    }

    if ipc_right_inuse(space, name, entry) != 0 {
        return KERN_NAME_EXISTS;
    }

    crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");
    (*entry).ie_bits |= MACH_PORT_TYPE_DEAD_NAME | 1;

    is_write_unlock(space);
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_object_alloc / ipc_object_alloc_name
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_alloc(
    space: ipc_space_t,
    otype: u32,
    typ: mach_port_type_t,
    urefs: mach_port_urefs_t,
    namep: *mut mach_port_name_t,
    objectp: *mut *mut ipc_object,
) -> kern_return_t {
    let mut entry: ipc_entry_t = IE_NULL;

    crate::kassert!(otype < IOT_NUMBER as u32, "otype < IOT_NUMBER");
    crate::kassert!(
        (typ & MACH_PORT_TYPE_ALL_RIGHTS) == typ,
        "(type & MACH_PORT_TYPE_ALL_RIGHTS) == type"
    );
    crate::kassert!(typ != MACH_PORT_TYPE_NONE, "type != MACH_PORT_TYPE_NONE");
    crate::kassert!(urefs <= MACH_PORT_UREFS_MAX, "urefs <= MACH_PORT_UREFS_MAX");
    let object = io_alloc(otype);
    if object.is_null() {
        return KERN_RESOURCE_SHORTAGE;
    }

    if otype == IOT_PORT {
        let port = object as *mut ipc_port;
        write_bytes(port, 0, 1);
    } else if otype == IOT_PORT_SET {
        let pset = object as *mut ipc_pset;
        write_bytes(pset, 0, 1);
    }

    is_write_lock(space);
    let kr = ipc_entry_alloc(space, namep, &mut entry);
    if kr != KERN_SUCCESS {
        is_write_unlock(space);
        io_free(otype, object);
        return kr;
    }

    (*entry).ie_bits |= typ | urefs;
    (*entry).ie_object = object;

    io_lock_init(object);
    io_lock(object);
    is_write_unlock(space);

    (*object).io_references = 1; /* for entry, not caller */
    (*object).io_bits = io_makebits(true, otype, 0);

    *objectp = object;
    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn ipc_object_alloc_name(
    space: ipc_space_t,
    otype: u32,
    typ: mach_port_type_t,
    urefs: mach_port_urefs_t,
    name: mach_port_name_t,
    objectp: *mut *mut ipc_object,
) -> kern_return_t {
    let mut entry: ipc_entry_t = IE_NULL;

    let object = io_alloc(otype);
    if object.is_null() {
        return KERN_RESOURCE_SHORTAGE;
    }

    if otype == IOT_PORT {
        let port = object as *mut ipc_port;
        write_bytes(port, 0, 1);
    } else if otype == IOT_PORT_SET {
        let pset = object as *mut ipc_pset;
        write_bytes(pset, 0, 1);
    }

    is_write_lock(space);
    let kr = ipc_entry_alloc_name(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        is_write_unlock(space);
        io_free(otype, object);
        return kr;
    }

    if ipc_right_inuse(space, name, entry) != 0 {
        io_free(otype, object);
        return KERN_NAME_EXISTS;
    }

    (*entry).ie_bits |= typ | urefs;
    (*entry).ie_object = object;

    io_lock_init(object);
    io_lock(object);
    is_write_unlock(space);

    (*object).io_references = 1; /* for entry, not caller */
    (*object).io_bits = io_makebits(true, otype, 0);

    *objectp = object;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_object_copyin_type
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_copyin_type(
    msgt_name: mach_msg_type_name_t,
) -> mach_msg_type_name_t {
    match msgt_name {
        0 => 0,
        x if x == MACH_MSG_TYPE_MOVE_RECEIVE => MACH_MSG_TYPE_PORT_RECEIVE,
        x if x == MACH_MSG_TYPE_MOVE_SEND_ONCE || x == MACH_MSG_TYPE_MAKE_SEND_ONCE => {
            MACH_MSG_TYPE_PORT_SEND_ONCE
        }
        x if x == MACH_MSG_TYPE_MOVE_SEND
            || x == MACH_MSG_TYPE_MAKE_SEND
            || x == MACH_MSG_TYPE_COPY_SEND =>
        {
            MACH_MSG_TYPE_PORT_SEND
        }
        _ => 0, /* in case assert/panic returns */
    }
}

// ---------------------------------------------------------------------------
//  ipc_object_copyin
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_copyin(
    space: ipc_space_t,
    name: mach_port_name_t,
    msgt_name: mach_msg_type_name_t,
    objectp: *mut *mut ipc_object,
) -> kern_return_t {
    let mut entry: ipc_entry_t = IE_NULL;
    let mut soright: *mut ipc_port = null_mut();

    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* space is write-locked and active */

    let kr = ipc_right_copyin(
        space,
        name,
        entry,
        msgt_name,
        1, /* TRUE */
        objectp,
        &mut soright,
    );
    if ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE {
        ipc_entry_dealloc(space, name, entry);
    }
    is_write_unlock(space);

    if kr == KERN_SUCCESS && !soright.is_null() {
        ipc_notify_port_deleted(soright, name);
    }

    kr
}

// ---------------------------------------------------------------------------
//  ipc_object_copyin_from_kernel
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_copyin_from_kernel(
    object: *mut ipc_object,
    msgt_name: mach_msg_type_name_t,
) {
    crate::kassert!(
        (object as usize) != 0 && (object as usize) != !0usize,
        "IO_VALID(object)"
    );

    if msgt_name == MACH_MSG_TYPE_MOVE_RECEIVE {
        let port = object as *mut ipc_port;

        ip_lock(port);
        crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!(
            (*port).ip_target.ipt_name != MACH_PORT_NAME_NULL,
            "port->ip_receiver_name != MACH_PORT_NULL"
        );
        crate::kassert!(
            (*port).data.receiver == ipc_space_kernel,
            "port->ip_receiver == ipc_space_kernel"
        );

        /* relevant part of ipc_port_clear_receiver */
        ipc_port_set_mscount(port, 0);

        (*port).ip_target.ipt_name = MACH_PORT_NAME_NULL;
        (*port).data.destination = null_mut();
        ipc_port_flag_protected_payload_clear(port);
        ip_unlock(port);
    } else if msgt_name == MACH_MSG_TYPE_COPY_SEND {
        let port = object as *mut ipc_port;

        ip_lock(port);
        if ip_active(port) {
            crate::kassert!((*port).ip_srights > 0, "port->ip_srights > 0");
            (*port).ip_srights += 1;
        }
        ip_reference(port);
        ip_unlock(port);
    } else if msgt_name == MACH_MSG_TYPE_MAKE_SEND {
        let port = object as *mut ipc_port;

        ip_lock(port);
        crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!(
            (*port).ip_target.ipt_name != MACH_PORT_NAME_NULL,
            "port->ip_receiver_name != MACH_PORT_NULL"
        );
        crate::kassert!(
            (*port).data.receiver == ipc_space_kernel,
            "port->ip_receiver == ipc_space_kernel"
        );

        ip_reference(port);
        (*port).ip_mscount += 1;
        (*port).ip_srights += 1;
        ip_unlock(port);
    } else if msgt_name == MACH_MSG_TYPE_MOVE_SEND {
        /* move naked send right into the message */
    } else if msgt_name == MACH_MSG_TYPE_MAKE_SEND_ONCE {
        let port = object as *mut ipc_port;

        ip_lock(port);
        crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!(
            (*port).ip_target.ipt_name != MACH_PORT_NAME_NULL,
            "port->ip_receiver_name != MACH_PORT_NULL"
        );
        crate::kassert!(
            (*port).data.receiver == ipc_space_kernel,
            "port->ip_receiver == ipc_space_kernel"
        );

        ip_reference(port);
        (*port).ip_sorights += 1;
        ip_unlock(port);
    } else if msgt_name == MACH_MSG_TYPE_MOVE_SEND_ONCE {
        /* move naked send-once right into the message */
    } else {
        crate::kpanic!("ipc_object_copyin_from_kernel: strange rights");
    }
}

// ---------------------------------------------------------------------------
//  ipc_object_destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_destroy(
    object: *mut ipc_object,
    msgt_name: mach_msg_type_name_t,
) {
    crate::kassert!(
        (object as usize) != 0 && (object as usize) != !0usize,
        "IO_VALID(object)"
    );
    crate::kassert!(io_otype(object) == IOT_PORT, "io_otype(object) == IOT_PORT");

    if msgt_name == MACH_MSG_TYPE_PORT_SEND {
        ipc_port_release_send(object as *mut ipc_port);
    } else if msgt_name == MACH_MSG_TYPE_PORT_SEND_ONCE {
        ipc_notify_send_once(object as *mut ipc_port);
    } else if msgt_name == MACH_MSG_TYPE_PORT_RECEIVE {
        ipc_port_release_receive(object as *mut ipc_port);
    } else {
        crate::kpanic!("ipc_object_destroy: strange rights");
    }
}

// ---------------------------------------------------------------------------
//  ipc_object_copyout
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_copyout(
    space: ipc_space_t,
    object: *mut ipc_object,
    msgt_name: mach_msg_type_name_t,
    overflow: crate::mach_types::boolean_t,
    namep: *mut mach_port_name_t,
) -> kern_return_t {
    let mut name: mach_port_name_t = 0;
    let mut entry: ipc_entry_t = IE_NULL;

    crate::kassert!(
        (object as usize) != 0 && (object as usize) != !0usize,
        "IO_VALID(object)"
    );
    crate::kassert!(io_otype(object) == IOT_PORT, "io_otype(object) == IOT_PORT");

    is_write_lock(space);

    loop {
        if (*space).is_active == 0 {
            is_write_unlock(space);
            return KERN_INVALID_TASK;
        }

        if msgt_name != MACH_MSG_TYPE_PORT_SEND_ONCE
            && ipc_right_reverse(space, object, &mut name, &mut entry) != 0
        {
            /* object is locked and active */
            crate::kassert!(
                ((*entry).ie_bits & MACH_PORT_TYPE_SEND_RECEIVE) != 0,
                "entry->ie_bits & MACH_PORT_TYPE_SEND_RECEIVE"
            );
            break;
        }

        let kr = ipc_entry_alloc(space, &mut name, &mut entry);
        if kr != KERN_SUCCESS {
            is_write_unlock(space);
            return kr;
        }

        crate::kassert!(
            ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE,
            "IE_BITS_TYPE(entry->ie_bits) == MACH_PORT_TYPE_NONE"
        );
        crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");

        io_lock(object);
        if !io_active(object) {
            io_unlock(object);
            ipc_entry_dealloc(space, name, entry);
            is_write_unlock(space);
            return KERN_INVALID_CAPABILITY;
        }

        (*entry).ie_object = object;
        break;
    }

    /* space is write-locked and active, object is locked and active */

    let kr = ipc_right_copyout(space, name, entry, msgt_name, overflow, object);
    /* object is unlocked */
    is_write_unlock(space);

    if kr == KERN_SUCCESS {
        *namep = name;
    }
    kr
}

// ---------------------------------------------------------------------------
//  ipc_object_copyout_name
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_copyout_name(
    space: ipc_space_t,
    object: *mut ipc_object,
    msgt_name: mach_msg_type_name_t,
    overflow: crate::mach_types::boolean_t,
    name: mach_port_name_t,
) -> kern_return_t {
    let mut oname: mach_port_name_t = 0;
    let mut oentry: ipc_entry_t = IE_NULL;
    let mut entry: ipc_entry_t = IE_NULL;

    crate::kassert!(
        (object as usize) != 0 && (object as usize) != !0usize,
        "IO_VALID(object)"
    );
    crate::kassert!(io_otype(object) == IOT_PORT, "io_otype(object) == IOT_PORT");

    is_write_lock(space);
    let kr = ipc_entry_alloc_name(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        is_write_unlock(space);
        return kr;
    }

    if msgt_name != MACH_MSG_TYPE_PORT_SEND_ONCE
        && ipc_right_reverse(space, object, &mut oname, &mut oentry) != 0
    {
        /* object is locked and active */

        if name != oname {
            io_unlock(object);

            if ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE {
                ipc_entry_dealloc(space, name, entry);
            }

            is_write_unlock(space);
            return KERN_RIGHT_EXISTS;
        }

        crate::kassert!(entry == oentry, "entry == oentry");
        crate::kassert!(
            ((*entry).ie_bits & MACH_PORT_TYPE_SEND_RECEIVE) != 0,
            "entry->ie_bits & MACH_PORT_TYPE_SEND_RECEIVE"
        );
    } else {
        if ipc_right_inuse(space, name, entry) != 0 {
            return KERN_NAME_EXISTS;
        }

        crate::kassert!(
            ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE,
            "IE_BITS_TYPE(entry->ie_bits) == MACH_PORT_TYPE_NONE"
        );
        crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");

        io_lock(object);
        if !io_active(object) {
            io_unlock(object);
            ipc_entry_dealloc(space, name, entry);
            is_write_unlock(space);
            return KERN_INVALID_CAPABILITY;
        }

        (*entry).ie_object = object;
    }

    /* space is write-locked and active, object is locked and active */

    let kr = ipc_right_copyout(space, name, entry, msgt_name, overflow, object);
    /* object is unlocked */
    is_write_unlock(space);
    kr
}

// ---------------------------------------------------------------------------
//  ipc_object_copyout_dest
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_copyout_dest(
    space: ipc_space_t,
    object: *mut ipc_object,
    msgt_name: mach_msg_type_name_t,
    namep: *mut mach_port_name_t,
) {
    let name: mach_port_name_t;

    crate::kassert!(
        (object as usize) != 0 && (object as usize) != !0usize,
        "IO_VALID(object)"
    );
    crate::kassert!(io_active(object), "io_active(object)");

    io_release(object);

    if msgt_name == MACH_MSG_TYPE_PORT_SEND {
        let port = object as *mut ipc_port;
        let mut nsrequest: *mut ipc_port = null_mut();
        let mut mscount: mach_port_mscount_t = 0;

        crate::kassert!((*port).ip_srights > 0, "port->ip_srights > 0");
        (*port).ip_srights -= 1;
        if (*port).ip_srights == 0 {
            nsrequest = (*port).ip_nsrequest;
            if !nsrequest.is_null() {
                (*port).ip_nsrequest = null_mut();
                mscount = (*port).ip_mscount;
            }
        }

        if (*port).data.receiver == space {
            name = (*port).ip_target.ipt_name;
        } else {
            name = MACH_PORT_NAME_NULL;
        }

        ip_unlock(port);

        if !nsrequest.is_null() {
            ipc_notify_no_senders(nsrequest, mscount);
        }
    } else if msgt_name == MACH_MSG_TYPE_PORT_SEND_ONCE {
        let port = object as *mut ipc_port;

        crate::kassert!((*port).ip_sorights > 0, "port->ip_sorights > 0");

        if (*port).data.receiver == space {
            /* quietly consume the send-once right */
            (*port).ip_sorights -= 1;
            name = (*port).ip_target.ipt_name;
            ip_unlock(port);
        } else {
            /*
             *  A very bizarre case.  The message was received, but
             *  before this copyout happened the space lost receive
             *  rights.  We can't quietly consume the soright out from
             *  underneath some other task, so generate a send-once
             *  notification.
             */
            ip_reference(port); /* restore ref */
            ip_unlock(port);

            ipc_notify_send_once(port);
            name = MACH_PORT_NAME_NULL;
        }
    } else {
        crate::kpanic!("ipc_object_copyout_dest: strange rights");
    }

    *namep = name;

    let _ = ipc_space_kernel; /* silence unused-import warning */
}

// ---------------------------------------------------------------------------
//  ipc_object_rename
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_object_rename(
    space: ipc_space_t,
    oname: mach_port_name_t,
    nname: mach_port_name_t,
) -> kern_return_t {
    let mut nentry: ipc_entry_t = IE_NULL;

    is_write_lock(space);
    let kr = ipc_entry_alloc_name(space, nname, &mut nentry);
    if kr != KERN_SUCCESS {
        is_write_unlock(space);
        return kr;
    }

    if ipc_right_inuse(space, nname, nentry) != 0 {
        /* space is unlocked */
        return KERN_NAME_EXISTS;
    }

    /* don't let ipc_entry_lookup see the uninitialized new entry */

    let oentry: ipc_entry_t;
    if oname == nname {
        ipc_entry_dealloc(space, nname, nentry);
        is_write_unlock(space);
        return KERN_INVALID_NAME;
    }
    oentry = ipc_entry_lookup(space, oname);
    if oentry == IE_NULL {
        ipc_entry_dealloc(space, nname, nentry);
        is_write_unlock(space);
        return KERN_INVALID_NAME;
    }

    let kr = ipc_right_rename(space, oname, oentry, nname, nentry);
    /* space is unlocked */
    kr
}
