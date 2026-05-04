//! Port of `ipc/ipc_marequest.c`.
//!
//! Hash table of `ipc_marequest` records used to deliver msg-accepted
//! notifications.

use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut};

use crate::extern_c::{
    kalloc, kmem_cache_alloc, kmem_cache_free, kmem_cache_init, lock_done,
    lock_write, rdxtree_lookup_common, Assert,
};
use crate::ipc_right::ipc_right_reverse;
use crate::ipc_port::ipc_port_lookup_notify;
use crate::ipc_space::ipc_space_destroy;
use crate::ipc_notify::ipc_notify_msg_accepted;
use crate::mach_types::{
    ipc_entry, ipc_entry_t, ipc_marequest_bucket, ipc_marequest_bucket_t,
    ipc_marequest_full, ipc_marequest_t, ipc_object, ipc_port,
    ipc_port_t, ipc_space_t, kmem_cache, mach_port_name_t, mach_msg_return_t,
    rdxtree_key_t, vm_offset_t, vm_size_t, IE_BITS_MAREQUEST, IE_BITS_TYPE_MASK,
    IE_NULL, IMARB_NULL, IMAR_NULL, IPC_MAREQUEST_SIZE, IP_NULL,
    MACH_MSG_SUCCESS, MACH_PORT_NAME_NULL, MACH_PORT_NULL,
    MACH_PORT_TYPE_SEND_RECEIVE, MACH_SEND_INVALID_NOTIFY,
    MACH_SEND_NOTIFY_IN_PROGRESS, MACH_SEND_NO_NOTIFY,
};

type ipc_marequest_index_t = u32;
type ipc_entry_bits_t = u32;

// ---------------------------------------------------------------------------
//  Globals (defined here, as in C)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static mut ipc_marequest_cache: kmem_cache = kmem_cache {
    _bytes: [0u8; crate::mach_types::SIZE_OF_KMEM_CACHE],
};

#[no_mangle]
pub static mut ipc_marequest_size: ipc_marequest_index_t = 0;
#[no_mangle]
pub static mut ipc_marequest_mask: ipc_marequest_index_t = 0;

#[no_mangle]
pub static mut ipc_marequest_table: ipc_marequest_bucket_t = IMARB_NULL;

// ---------------------------------------------------------------------------
//  Macros expanded inline
// ---------------------------------------------------------------------------

#[inline]
unsafe fn imar_alloc() -> ipc_marequest_t {
    kmem_cache_alloc(addr_of_mut!(ipc_marequest_cache)) as ipc_marequest_t
}

#[inline]
unsafe fn imar_free(imar: ipc_marequest_t) {
    kmem_cache_free(addr_of_mut!(ipc_marequest_cache), imar as vm_offset_t);
}

/// `IMAR_HASH(space, name)` from C.  With the current port.h:
///   `MACH_PORT_INDEX(name) = name; MACH_PORT_NGEN(name) = 0;`
#[inline]
unsafe fn imar_hash(space: ipc_space_t, name: mach_port_name_t) -> u32 {
    (((space as u32) >> 4) + name + 0) & ipc_marequest_mask
}

#[inline]
unsafe fn imarb_lock_init(_b: ipc_marequest_bucket_t) {
    /* simple_lock_init: no-op with NCPUS == 1. */
}
#[inline]
unsafe fn imarb_lock(_b: ipc_marequest_bucket_t) {}
#[inline]
unsafe fn imarb_unlock(_b: ipc_marequest_bucket_t) {}

/// is_write_lock(space) → lock_write(&space->is_lock_data)
#[inline]
unsafe fn is_write_lock(space: ipc_space_t) {
    lock_write(addr_of_mut!((*space).is_lock_data));
}

/// is_write_unlock(space) → lock_done(&space->is_lock_data)
#[inline]
unsafe fn is_write_unlock(space: ipc_space_t) {
    lock_done(addr_of_mut!((*space).is_lock_data));
}

/// is_reference(space) → ipc_space_reference_macro(space).
/// With NCPUS=1 simple_lock/unlock are no-ops.
#[inline]
unsafe fn is_reference(space: ipc_space_t) {
    if (*space).is_references == 0 {
        Assert(
            b"rust/ipc/ipc_marequest.rs\0".as_ptr(),
            0,
            b"is_reference\0".as_ptr(),
            b"is_references > 0\0".as_ptr(),
        );
    }
    (*space).is_references += 1;
}

/// is_release(space) → ipc_space_release_macro(space).
#[inline]
unsafe fn is_release(space: ipc_space_t) {
    if (*space).is_references == 0 {
        Assert(
            b"rust/ipc/ipc_marequest.rs\0".as_ptr(),
            0,
            b"is_release\0".as_ptr(),
            b"is_references > 0\0".as_ptr(),
        );
    }
    (*space).is_references -= 1;
    let refs = (*space).is_references;
    if refs == 0 {
        ipc_space_destroy(space);
    }
}

#[inline]
unsafe fn ip_unlock(_port: *mut ipc_port) {
    /* io_unlock(&port->ip_object) → simple_unlock(...): no-op with NCPUS==1. */
}

/// Mirror of the static inline `ipc_entry_lookup` from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_lookup(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> ipc_entry_t {
    if (*space).is_active == 0 {
        Assert(
            b"rust/ipc/ipc_marequest.rs\0".as_ptr(),
            0,
            b"ipc_entry_lookup\0".as_ptr(),
            b"space->is_active\0".as_ptr(),
        );
    }
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

#[inline]
fn ie_bits_type(bits: ipc_entry_bits_t) -> ipc_entry_bits_t {
    bits & IE_BITS_TYPE_MASK
}

// ---------------------------------------------------------------------------
//  ipc_marequest_init
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_marequest_init() {
    /* initialize ipc_marequest_size */
    ipc_marequest_size = IPC_MAREQUEST_SIZE;

    /* make sure it is a power of two */
    ipc_marequest_mask = ipc_marequest_size - 1;
    if (ipc_marequest_size & ipc_marequest_mask) != 0 {
        let mut bit: u32 = 1;
        loop {
            ipc_marequest_mask |= bit;
            ipc_marequest_size = ipc_marequest_mask + 1;
            if (ipc_marequest_size & ipc_marequest_mask) == 0 {
                break;
            }
            bit <<= 1;
        }
    }

    /* allocate ipc_marequest_table */
    ipc_marequest_table = kalloc(
        ipc_marequest_size * size_of::<ipc_marequest_bucket>() as vm_size_t,
    ) as ipc_marequest_bucket_t;
    if ipc_marequest_table == IMARB_NULL {
        Assert(
            b"rust/ipc/ipc_marequest.rs\0".as_ptr(),
            0,
            b"ipc_marequest_init\0".as_ptr(),
            b"ipc_marequest_table != IMARB_NULL\0".as_ptr(),
        );
    }

    /* and initialize it */
    let mut i: ipc_marequest_index_t = 0;
    while i < ipc_marequest_size {
        let bucket = ipc_marequest_table.add(i as usize);
        imarb_lock_init(bucket);
        (*bucket).imarb_head = IMAR_NULL;
        i += 1;
    }

    kmem_cache_init(
        addr_of_mut!(ipc_marequest_cache),
        b"ipc_marequest\0".as_ptr() as *const _,
        size_of::<ipc_marequest_full>(),
        0,
        None,
        0,
    );
}

// ---------------------------------------------------------------------------
//  ipc_marequest_create
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_marequest_create(
    space: ipc_space_t,
    port: ipc_port_t,
    notify: mach_port_name_t,
    marequestp: *mut ipc_marequest_t,
) -> mach_msg_return_t {
    let mut name: mach_port_name_t = 0;
    let mut entry: ipc_entry_t = IE_NULL;
    let soright: ipc_port_t;
    let marequest: ipc_marequest_t;
    let bucket: ipc_marequest_bucket_t;

    marequest = imar_alloc();
    if marequest == IMAR_NULL {
        return MACH_SEND_NO_NOTIFY;
    }

    /*
     * Delay creating the send-once right until we know there will be no
     * errors.  Otherwise, we would have to worry about disposing of it
     * when it turned out it wasn't needed.
     */

    is_write_lock(space);
    if (*space).is_active == 0 {
        is_write_unlock(space);
        imar_free(marequest);
        return MACH_SEND_INVALID_NOTIFY;
    }

    if ipc_right_reverse(space, port as *mut ipc_object, &mut name, &mut entry) != 0 {
        let bits: ipc_entry_bits_t;

        /* port is locked and active */
        ip_unlock(port);
        bits = (*entry).ie_bits;

        crate::kassert!(port == (*entry).ie_object as ipc_port_t, "port == entry->ie_object");
        crate::kassert!((bits & MACH_PORT_TYPE_SEND_RECEIVE) != 0, "bits & MACH_PORT_TYPE_SEND_RECEIVE");

        if bits & IE_BITS_MAREQUEST != 0 {
            is_write_unlock(space);
            imar_free(marequest);
            return MACH_SEND_NOTIFY_IN_PROGRESS;
        }

        let so = ipc_port_lookup_notify(space, notify);
        if so == IP_NULL {
            is_write_unlock(space);
            imar_free(marequest);
            return MACH_SEND_INVALID_NOTIFY;
        }
        soright = so;

        (*entry).ie_bits = bits | IE_BITS_MAREQUEST;

        is_reference(space);
        (*marequest).imar_space = space;
        (*marequest).imar_name = name;
        (*marequest).imar_soright = soright;

        bucket = ipc_marequest_table.add(imar_hash(space, name) as usize);
        imarb_lock(bucket);

        (*marequest).imar_next = (*bucket).imarb_head;
        (*bucket).imarb_head = marequest;

        imarb_unlock(bucket);
    } else {
        let so = ipc_port_lookup_notify(space, notify);
        if so == IP_NULL {
            is_write_unlock(space);
            imar_free(marequest);
            return MACH_SEND_INVALID_NOTIFY;
        }
        soright = so;

        is_reference(space);
        (*marequest).imar_space = space;
        (*marequest).imar_name = MACH_PORT_NULL;
        (*marequest).imar_soright = soright;
    }

    is_write_unlock(space);
    *marequestp = marequest;
    let _ = MACH_PORT_TYPE_SEND_RECEIVE; // silence unused-import
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_marequest_cancel
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_marequest_cancel(
    space: ipc_space_t,
    name: mach_port_name_t,
) {
    let bucket: ipc_marequest_bucket_t;
    let mut marequest: ipc_marequest_t;
    let mut last: *mut ipc_marequest_t;

    crate::kassert!((*space).is_active != 0, "space->is_active");

    bucket = ipc_marequest_table.add(imar_hash(space, name) as usize);
    imarb_lock(bucket);

    last = addr_of_mut!((*bucket).imarb_head);
    loop {
        marequest = *last;
        if marequest == IMAR_NULL {
            break;
        }
        if (*marequest).imar_space == space && (*marequest).imar_name == name {
            break;
        }
        last = addr_of_mut!((*marequest).imar_next);
    }

    crate::kassert!(marequest != IMAR_NULL, "marequest != IMAR_NULL");
    *last = (*marequest).imar_next;
    imarb_unlock(bucket);

    (*marequest).imar_name = MACH_PORT_NAME_NULL;
}

// ---------------------------------------------------------------------------
//  ipc_marequest_rename
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_marequest_rename(
    space: ipc_space_t,
    old: mach_port_name_t,
    new: mach_port_name_t,
) {
    let mut bucket: ipc_marequest_bucket_t;
    let mut marequest: ipc_marequest_t;
    let mut last: *mut ipc_marequest_t;

    bucket = ipc_marequest_table.add(imar_hash(space, old) as usize);
    imarb_lock(bucket);

    last = addr_of_mut!((*bucket).imarb_head);
    loop {
        marequest = *last;
        if marequest == IMAR_NULL {
            break;
        }
        if (*marequest).imar_space == space && (*marequest).imar_name == old {
            break;
        }
        last = addr_of_mut!((*marequest).imar_next);
    }

    *last = (*marequest).imar_next;
    imarb_unlock(bucket);

    (*marequest).imar_name = new;

    bucket = ipc_marequest_table.add(imar_hash(space, new) as usize);
    imarb_lock(bucket);

    (*marequest).imar_next = (*bucket).imarb_head;
    (*bucket).imarb_head = marequest;

    imarb_unlock(bucket);
}

// ---------------------------------------------------------------------------
//  ipc_marequest_destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_marequest_destroy(marequest: ipc_marequest_t) {
    let space = (*marequest).imar_space;
    let mut name: mach_port_name_t;
    let soright: ipc_port_t;

    is_write_lock(space);

    name = (*marequest).imar_name;
    soright = (*marequest).imar_soright;

    if name != MACH_PORT_NULL {
        let bucket: ipc_marequest_bucket_t;
        let mut this: ipc_marequest_t;
        let mut last: *mut ipc_marequest_t;

        bucket = ipc_marequest_table.add(imar_hash(space, name) as usize);
        imarb_lock(bucket);

        last = addr_of_mut!((*bucket).imarb_head);
        loop {
            this = *last;
            if this == IMAR_NULL {
                break;
            }
            if (*this).imar_space == space && (*this).imar_name == name {
                break;
            }
            last = addr_of_mut!((*this).imar_next);
        }

        crate::kassert!(this == marequest, "this == marequest");
        *last = (*this).imar_next;
        imarb_unlock(bucket);

        if (*space).is_active != 0 {
            let entry: ipc_entry_t = ipc_entry_lookup(space, name);
            crate::kassert!(entry != IE_NULL, "entry != IE_NULL");
            crate::kassert!(((*entry).ie_bits & IE_BITS_MAREQUEST) != 0, "entry->ie_bits & IE_BITS_MAREQUEST");
            crate::kassert!(((*entry).ie_bits & MACH_PORT_TYPE_SEND_RECEIVE) != 0, "entry->ie_bits & MACH_PORT_TYPE_SEND_RECEIVE");

            (*entry).ie_bits &= !IE_BITS_MAREQUEST;
        } else {
            name = MACH_PORT_NAME_NULL;
        }
    }

    is_write_unlock(space);
    is_release(space);

    imar_free(marequest);

    crate::kassert!(soright != IP_NULL, "soright != IP_NULL");
    ipc_notify_msg_accepted(soright, name);
    let _ = ie_bits_type;
}

// ---------------------------------------------------------------------------
//  ipc_marequest_info
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct hash_info_bucket_t {
    pub hib_count: u32,
}

#[no_mangle]
pub unsafe extern "C" fn ipc_marequest_info(
    maxp: *mut u32,
    info: *mut hash_info_bucket_t,
    mut count: u32,
) -> u32 {
    if ipc_marequest_size < count {
        count = ipc_marequest_size;
    }

    let mut i: ipc_marequest_index_t = 0;
    while i < count {
        let bucket = ipc_marequest_table.add(i as usize);
        let mut bucket_count: u32 = 0;
        let mut marequest: ipc_marequest_t;

        imarb_lock(bucket);
        marequest = (*bucket).imarb_head;
        while marequest != IMAR_NULL {
            bucket_count += 1;
            marequest = (*marequest).imar_next;
        }
        imarb_unlock(bucket);

        /* don't touch pageable memory while holding locks */
        (*info.add(i as usize)).hib_count = bucket_count;
        i += 1;
    }

    *maxp = u32::MAX;
    ipc_marequest_size
}
