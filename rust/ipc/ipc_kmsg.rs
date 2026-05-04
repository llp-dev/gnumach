//! Port of `ipc/ipc_kmsg.c`.  Operations on kernel messages.
//!
//! Functions are migrated one at a time.  Each ported function is removed
//! from the C `ipc/ipc_kmsg.c` (which remains in the build with the
//! still-unported functions).

use core::ptr::addr_of_mut;

use crate::extern_c::{
    copyin, copyinmap, copyinmsg, copyout, copyoutmap, copyoutmsg, kalloc, kfree, percpu_array,
    vm_allocate, vm_deallocate, vm_map_copy_discard, vm_map_copyin, vm_map_copyin_page_list,
    vm_map_copyout,
};

/// `ipc/ipc_kmsg.c`: per-CPU kmsg cache (NCPUS == 1, length 1).
#[no_mangle]
pub static mut ipc_kmsg_cache: [*mut crate::mach_types::ipc_kmsg_full; 1] = [core::ptr::null_mut()];
use crate::mach_types::{ipc_space_t, mach_port_name_t, vm_map_t};

use crate::ipc_entry::ipc_entry_alloc;
use crate::ipc_marequest::ipc_marequest_destroy;
use crate::ipc_notify::{
    ipc_notify_dead_name, ipc_notify_no_senders, ipc_notify_port_deleted, ipc_notify_send_once,
};
use crate::ipc_object::{
    ipc_object_copyin, ipc_object_copyin_from_kernel, ipc_object_copyin_type, ipc_object_copyout,
    ipc_object_copyout_dest, ipc_object_destroy,
};
use crate::ipc_port::{
    ipc_port_check_circularity, ipc_port_copy_send, ipc_port_dngrow, ipc_port_dnrequest,
    ipc_port_lookup_notify, ipc_port_release_sonce,
};
use crate::ipc_right::{
    ipc_right_copyin, ipc_right_copyin_check, ipc_right_copyin_two, ipc_right_copyin_undo,
    ipc_right_copyout, ipc_right_reverse,
};
use crate::mach_types::{
    ipc_kmsg_full, ipc_kmsg_queue, ipc_object, ipc_thread_t, mach_msg_return_t,
    mach_msg_type_long_t, mach_msg_type_t, vm_map_copy_t, vm_offset_t, vm_size_t,
    IE_BITS_MAREQUEST, IE_BITS_TYPE_MASK, IE_BITS_UREFS_MASK, IKM_EXPAND_FACTOR, IKM_NULL,
    IKM_OVERHEAD, IKM_SAVED_KMSG_SIZE, IKM_SAVED_MSG_SIZE, IKOT_DEVICE, IKOT_PAGING_REQUEST,
    IKOT_USER_DEVICE, IMAR_NULL, IO_BITS_PROTECTED_PAYLOAD, IO_DEAD, IP_NULL, KERN_FAILURE,
    KERN_INVALID_CAPABILITY, KERN_NO_SPACE, KERN_RESOURCE_SHORTAGE, KERN_SUCCESS, MACH_MSGH_BITS,
    MACH_MSGH_BITS_CIRCULAR, MACH_MSGH_BITS_COMPLEX, MACH_MSGH_BITS_LOCAL, MACH_MSGH_BITS_OTHER,
    MACH_MSGH_BITS_PORTS, MACH_MSG_IPC_KERNEL, MACH_MSG_IPC_SPACE, MACH_MSG_KERNEL_ALIGNMENT,
    MACH_MSG_SUCCESS, MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND,
    MACH_MSG_TYPE_MAKE_SEND_ONCE, MACH_MSG_TYPE_MOVE_SEND, MACH_MSG_TYPE_MOVE_SEND_ONCE,
    MACH_MSG_TYPE_PORT_ANY, MACH_MSG_TYPE_PORT_ANY_SEND, MACH_MSG_TYPE_PORT_RECEIVE,
    MACH_MSG_TYPE_PORT_SEND, MACH_MSG_TYPE_PORT_SEND_ONCE, MACH_MSG_TYPE_PROTECTED_PAYLOAD,
    MACH_MSG_USER_ALIGNMENT, MACH_MSG_VM_KERNEL, MACH_MSG_VM_SPACE, MACH_PORT_NAME_DEAD,
    MACH_PORT_NAME_NULL, MACH_PORT_TYPE_DEAD_NAME, MACH_PORT_TYPE_NONE, MACH_PORT_TYPE_RECEIVE,
    MACH_PORT_TYPE_SEND, MACH_PORT_TYPE_SEND_ONCE, MACH_PORT_TYPE_SEND_RECEIVE,
    MACH_PORT_UREFS_MAX, MACH_RCV_BODY_ERROR, MACH_RCV_HEADER_ERROR, MACH_RCV_INVALID_DATA,
    MACH_RCV_INVALID_NOTIFY, MACH_SEND_INVALID_DATA, MACH_SEND_INVALID_DEST,
    MACH_SEND_INVALID_HEADER, MACH_SEND_INVALID_MEMORY, MACH_SEND_INVALID_NOTIFY,
    MACH_SEND_INVALID_REPLY, MACH_SEND_INVALID_RIGHT, MACH_SEND_INVALID_TYPE,
    MACH_SEND_MSG_TOO_SMALL, MACH_SEND_NO_BUFFER, OFFSETOF_PERCPU_ACTIVE_THREAD,
    PORT_NAME_T_SIZE_IN_BITS, PORT_T_SIZE_IN_BITS, VM_MIN_KERNEL_ADDRESS,
};

/// `current_thread()` — `percpu_array[0].active_thread` on NCPUS == 1.
#[inline]
unsafe fn current_thread() -> ipc_thread_t {
    let base = addr_of_mut!(percpu_array) as *mut u8;
    *(base.add(OFFSETOF_PERCPU_ACTIVE_THREAD) as *mut ipc_thread_t)
}

/// `mach_msg_kernel_align(x) = (x + 3) & ~3`.
#[inline]
const fn mach_msg_kernel_align(x: vm_offset_t) -> vm_offset_t {
    (x + (MACH_MSG_KERNEL_ALIGNMENT as vm_offset_t - 1))
        & !(MACH_MSG_KERNEL_ALIGNMENT as vm_offset_t - 1)
}

/// `mach_msg_kernel_is_misaligned(x) = (x & 3)`.
#[inline]
const fn mach_msg_kernel_is_misaligned(x: vm_offset_t) -> bool {
    (x & (MACH_MSG_KERNEL_ALIGNMENT as vm_offset_t - 1)) != 0
}

/// `IO_VALID(io) = (io != IO_NULL && io != IO_DEAD)`.
#[inline]
unsafe fn io_valid(io: *mut ipc_object) -> bool {
    !io.is_null() && (io as usize) != !0usize
}

/// `invalid_port_to_name(port)` — see `ipc/port.h`: `MACH_PORT_NULL` →
/// `MACH_PORT_NAME_NULL`, `MACH_PORT_DEAD` → `MACH_PORT_NAME_DEAD`,
/// otherwise panic.
#[inline]
unsafe fn invalid_port_to_name(port: *mut ipc_object) -> mach_port_name_t {
    let p = port as usize;
    if p == 0 {
        MACH_PORT_NAME_NULL
    } else if p == !0usize {
        MACH_PORT_NAME_DEAD
    } else {
        crate::kpanic!("invalid_port_to_name() called with a valid port");
    }
}

use crate::locks::{
    io_active, io_check_unlock, io_lock, io_release, io_unlock, ip_active, ip_lock as ip_lock_noop,
    ip_release, ip_unlock as ip_unlock_noop, is_read_lock, is_read_unlock, is_write_lock,
    is_write_unlock,
};

/// `KEY(X) = ((X - VM_MIN_KERNEL_ADDRESS) >> 3)` from `ipc_space.h`.
#[inline]
fn rdxtree_key_of(obj: *mut ipc_object) -> crate::mach_types::rdxtree_key_t {
    (((obj as u32).wrapping_sub(VM_MIN_KERNEL_ADDRESS)) >> 3) as crate::mach_types::rdxtree_key_t
}

/// `ipc_reverse_lookup` from `ipc/ipc_space.h` (static inline).
#[inline]
unsafe fn ipc_reverse_lookup(
    space: ipc_space_t,
    obj: *mut ipc_object,
) -> crate::mach_types::ipc_entry_t {
    crate::extern_c::rdxtree_lookup_common(
        core::ptr::addr_of!((*space).is_reverse_map),
        rdxtree_key_of(obj),
        0,
    ) as crate::mach_types::ipc_entry_t
}

#[inline]
fn ie_bits_urefs(bits: u32) -> u32 {
    bits & IE_BITS_UREFS_MASK
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyout_body — outbound body walker (port rights + OOL memory).
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyout_body(
    kmsg: *mut ipc_kmsg_full,
    space: ipc_space_t,
    map: vm_map_t,
) -> mach_msg_return_t {
    let mut mr: mach_msg_return_t = MACH_MSG_SUCCESS;

    let mut saddr: vm_offset_t = addr_of_mut!((*kmsg).ikm_header).add(1) as vm_offset_t;
    let eaddr: vm_offset_t = (addr_of_mut!((*kmsg).ikm_header) as vm_offset_t)
        + (*kmsg).ikm_header.msgh_size as vm_offset_t;

    while saddr < eaddr {
        let taddr: vm_offset_t = saddr;
        let typ_ptr = saddr as *mut mach_msg_type_long_t;
        let typ_short = saddr as *mut mach_msg_type_t;

        let is_inline = (*typ_short).msgt_inline() != 0;
        let longform = (*typ_short).msgt_longform() != 0;
        let name: u32;
        let size: u32;
        let number: u32;

        if longform {
            name = (*typ_ptr).msgtl_name as u32;
            size = (*typ_ptr).msgtl_size as u32;
            number = (*typ_ptr).msgtl_number as u32;
            saddr += core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(
                core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t
            ) {
                saddr = mach_msg_kernel_align(saddr);
            }
        } else {
            name = (*typ_short).msgt_name();
            size = (*typ_short).msgt_size();
            number = (*typ_short).msgt_number();
            saddr += core::mem::size_of::<mach_msg_type_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(core::mem::size_of::<mach_msg_type_t>() as vm_offset_t)
            {
                saddr = mach_msg_kernel_align(saddr);
            }
        }

        /* calculate length of data in bytes, rounding up */
        let length: vm_size_t = (((number as u64) * (size as u64) + 7) >> 3) as vm_size_t;

        let is_port = MACH_MSG_TYPE_PORT_ANY(name);

        let mut addr: vm_offset_t = 0;
        let mut copyout_failed: bool = false;
        let mut kr: crate::mach_types::kern_return_t = KERN_SUCCESS;

        if is_port {
            if !is_inline {
                if length != 0 {
                    let user_length: vm_size_t = if core::mem::size_of::<mach_port_name_t>()
                        != core::mem::size_of::<vm_offset_t>()
                    {
                        (core::mem::size_of::<mach_port_name_t>() * number as usize) as vm_size_t
                    } else {
                        length
                    };

                    /* first allocate memory in the map */
                    kr = vm_allocate(map, &mut addr, user_length, 1 /* TRUE */);
                    if kr != KERN_SUCCESS {
                        ipc_kmsg_clean_body(taddr, saddr);
                        copyout_failed = true;
                    }
                }

                if !copyout_failed
                    && core::mem::size_of::<mach_port_name_t>()
                        != core::mem::size_of::<vm_offset_t>()
                {
                    /* OOL ports always returned as mach_port_name_t. */
                    (*typ_ptr).msgtl_size = (core::mem::size_of::<mach_port_name_t>() * 8) as u16;
                }
            }

            if !copyout_failed {
                let objects: *mut *mut ipc_object = if is_inline {
                    saddr as *mut *mut ipc_object
                } else {
                    *(saddr as *mut vm_offset_t) as *mut *mut ipc_object
                };

                /* copyout port rights carried in the message */
                let mut i: u32 = 0;
                while i < number {
                    let object = *objects.add(i as usize);
                    let mut name_out: mach_port_name_t = 0;
                    mr |= ipc_kmsg_copyout_object(space, object, name, &mut name_out);
                    /* `ipc_kmsg_copyout_object_to_port` writes back as a
                     * mach_port_t which on i686 is the same 4-byte width as
                     * mach_port_name_t. */
                    *(objects as *mut mach_port_name_t).add(i as usize) = name_out;
                    i += 1;
                }
            }
        }

        if is_inline {
            (*typ_short).set_msgt_deallocate(0 /* FALSE */);
            saddr += length;
        } else {
            let data: vm_offset_t = *(saddr as *mut vm_offset_t);

            /* copyout memory carried in the message */
            if !copyout_failed {
                if length == 0 {
                    crate::kassert!(data == 0, "data == 0");
                    addr = 0;
                } else if is_port {
                    /* copyout to memory allocated above */
                    if core::mem::size_of::<mach_port_name_t>()
                        != core::mem::size_of::<vm_offset_t>()
                    {
                        let src = data as *const u32;
                        let dst = addr as *mut u32;
                        let mut i: u32 = 0;
                        while i < number {
                            if copyout(
                                src.add(i as usize) as *const core::ffi::c_void,
                                dst.add(i as usize) as *mut core::ffi::c_void,
                                core::mem::size_of::<vm_offset_t>(),
                            ) != 0
                            {
                                kr = KERN_FAILURE;
                                copyout_failed = true;
                                break;
                            }
                            i += 1;
                        }
                    } else {
                        let _ = copyoutmap(
                            map,
                            data as *const core::ffi::c_char,
                            addr as *mut core::ffi::c_char,
                            length as core::ffi::c_int,
                        );
                    }
                    if !copyout_failed {
                        kfree(data, length);
                    }
                } else {
                    let copy = data as vm_map_copy_t;
                    kr = vm_map_copyout(map, &mut addr, copy);
                    if kr != KERN_SUCCESS {
                        vm_map_copy_discard(copy);
                        copyout_failed = true;
                    }
                }
            }

            if copyout_failed {
                /* vm_copyout_failure: */
                addr = 0;
                if longform {
                    (*typ_ptr).msgtl_size = 0;
                } else {
                    (*typ_short).set_msgt_size(0);
                }

                if kr == KERN_RESOURCE_SHORTAGE {
                    mr |= MACH_MSG_VM_KERNEL;
                } else {
                    mr |= MACH_MSG_VM_SPACE;
                }
            }

            (*typ_short).set_msgt_deallocate(1 /* TRUE */);
            *(saddr as *mut vm_offset_t) = addr;
            saddr += core::mem::size_of::<vm_offset_t>() as vm_offset_t;
        }

        /* Next element is always correctly aligned */
        saddr = mach_msg_kernel_align(saddr);
    }

    mr
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyout_dest — copy out the destination port, destroy reply.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyout_dest(kmsg: *mut ipc_kmsg_full, space: ipc_space_t) {
    let mbits: u32 = (*kmsg).ikm_header.msgh_bits;
    let dest: *mut ipc_object = (*kmsg).ikm_header.msgh_remote_port as *mut ipc_object;
    let reply: *mut ipc_object = (*kmsg).ikm_header.msgh_local_port as *mut ipc_object;
    let dest_type: u32 = mbits & 0xff; /* MACH_MSGH_BITS_REMOTE(mbits) */
    let reply_type: u32 = MACH_MSGH_BITS_LOCAL(mbits);

    crate::kassert!(io_valid(dest), "IO_VALID(dest)");

    let dest_name: mach_port_name_t;
    let reply_name: mach_port_name_t;

    io_lock(dest);
    if io_active(dest) {
        let mut name: mach_port_name_t = 0;
        ipc_object_copyout_dest(space, dest, dest_type, &mut name);
        /* dest is unlocked */
        dest_name = name;
    } else {
        io_release(dest);
        io_check_unlock(dest);
        dest_name = MACH_PORT_NAME_DEAD;
    }

    if io_valid(reply) {
        ipc_object_destroy(reply, reply_type);
        reply_name = MACH_PORT_NAME_NULL;
    } else {
        reply_name = invalid_port_to_name(reply);
    }

    (*kmsg).ikm_header.msgh_bits =
        MACH_MSGH_BITS_OTHER(mbits) | MACH_MSGH_BITS(reply_type, dest_type);
    (*kmsg).ikm_header.msgh_local_port = dest_name;
    (*kmsg).ikm_header.msgh_remote_port = reply_name;

    if (mbits & MACH_MSGH_BITS_COMPLEX) != 0 {
        let saddr: vm_offset_t = addr_of_mut!((*kmsg).ikm_header).add(1) as vm_offset_t;
        let eaddr: vm_offset_t = (addr_of_mut!((*kmsg).ikm_header) as vm_offset_t)
            + (*kmsg).ikm_header.msgh_size as vm_offset_t;

        ipc_kmsg_clean_body(saddr, eaddr);
    }

    let _ = io_unlock; /* silence unused-import */
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyout — copyout port rights + out-of-line memory.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyout(
    kmsg: *mut ipc_kmsg_full,
    space: ipc_space_t,
    map: vm_map_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    let mbits: u32 = (*kmsg).ikm_header.msgh_bits;

    let mut mr = ipc_kmsg_copyout_header(addr_of_mut!((*kmsg).ikm_header), space, notify);
    if mr != MACH_MSG_SUCCESS {
        return mr;
    }

    if (mbits & MACH_MSGH_BITS_COMPLEX) != 0 {
        mr = ipc_kmsg_copyout_body(kmsg, space, map);
        if mr != MACH_MSG_SUCCESS {
            mr |= MACH_RCV_BODY_ERROR;
        }
    }

    mr
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyout_pseudo — pseudo-copyout (headers handled as body).
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyout_pseudo(
    kmsg: *mut ipc_kmsg_full,
    space: ipc_space_t,
    map: vm_map_t,
) -> mach_msg_return_t {
    let mbits: u32 = (*kmsg).ikm_header.msgh_bits;
    let dest: *mut ipc_object = (*kmsg).ikm_header.msgh_remote_port as *mut ipc_object;
    let reply: *mut ipc_object = (*kmsg).ikm_header.msgh_local_port as *mut ipc_object;
    let dest_type: u32 = mbits & 0xff; /* MACH_MSGH_BITS_REMOTE(mbits) */
    let reply_type: u32 = MACH_MSGH_BITS_LOCAL(mbits);

    crate::kassert!(io_valid(dest), "IO_VALID(dest)");

    let mut dest_name: mach_port_name_t = 0;
    let mut reply_name: mach_port_name_t = 0;
    let mut mr: mach_msg_return_t = ipc_kmsg_copyout_object(space, dest, dest_type, &mut dest_name)
        | ipc_kmsg_copyout_object(space, reply, reply_type, &mut reply_name);

    (*kmsg).ikm_header.msgh_bits = mbits & !MACH_MSGH_BITS_CIRCULAR;
    (*kmsg).ikm_header.msgh_remote_port = dest_name;
    (*kmsg).ikm_header.msgh_local_port = reply_name;

    if (mbits & MACH_MSGH_BITS_COMPLEX) != 0 {
        mr |= ipc_kmsg_copyout_body(kmsg, space, map);
    }

    mr
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyout_object — fast-path copyout of a port right.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyout_object(
    space: ipc_space_t,
    object: *mut ipc_object,
    msgt_name: u32,
    namep: *mut mach_port_name_t,
) -> mach_msg_return_t {
    if !io_valid(object) {
        *namep = invalid_port_to_name(object);
        return MACH_MSG_SUCCESS;
    }

    /*
     *  Attempt quick copyout of send rights.  We optimize for a live port
     *  for which the receiver holds send (and not receive) rights in his
     *  local table.
     */
    'fast: {
        if msgt_name != MACH_MSG_TYPE_PORT_SEND {
            break 'fast;
        }

        let port = object as crate::mach_types::ipc_port_t;
        is_write_lock(space);
        if (*space).is_active == 0 {
            is_write_unlock(space);
            break 'fast;
        }

        ip_lock_noop(port);
        let entry = ipc_reverse_lookup(space, port as *mut ipc_object);
        if !ip_active(port) || entry.is_null() {
            ip_unlock_noop(port);
            is_write_unlock(space);
            break 'fast;
        }
        *namep = (*entry).ie_name;

        /*
         *  Copyout the send right, incrementing urefs unless it would
         *  overflow, and consume the right.
         */
        crate::kassert!((*port).ip_srights > 1, "port->ip_srights > 1");
        (*port).ip_srights -= 1;
        ip_release(port);
        ip_unlock_noop(port);

        crate::kassert!(
            ((*entry).ie_bits & MACH_PORT_TYPE_SEND) != 0,
            "entry->ie_bits & MACH_PORT_TYPE_SEND"
        );
        crate::kassert!(
            ie_bits_urefs((*entry).ie_bits) > 0,
            "IE_BITS_UREFS(entry->ie_bits) > 0"
        );
        crate::kassert!(
            ie_bits_urefs((*entry).ie_bits) < MACH_PORT_UREFS_MAX,
            "IE_BITS_UREFS(entry->ie_bits) < MACH_PORT_UREFS_MAX"
        );

        let bits: u32 = (*entry).ie_bits + 1;
        if ie_bits_urefs(bits) < MACH_PORT_UREFS_MAX {
            (*entry).ie_bits = bits;
        }

        is_write_unlock(space);
        return MACH_MSG_SUCCESS;
    }

    /* slow_copyout: */
    let kr = ipc_object_copyout(space, object, msgt_name, 1 /* TRUE */, namep);
    if kr != crate::mach_types::KERN_SUCCESS {
        ipc_object_destroy(object, msgt_name);

        if kr == KERN_INVALID_CAPABILITY {
            *namep = MACH_PORT_NAME_DEAD;
        } else {
            *namep = MACH_PORT_NAME_NULL;
            if kr == KERN_RESOURCE_SHORTAGE {
                return MACH_MSG_IPC_KERNEL;
            } else {
                return MACH_MSG_IPC_SPACE;
            }
        }
    }
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyin_from_kernel — kernel-side copyin (no error paths).
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyin_from_kernel(kmsg: *mut ipc_kmsg_full) {
    let mut bits: u32 = (*kmsg).ikm_header.msgh_bits;
    let rname: u32 = bits & 0xff; /* MACH_MSGH_BITS_REMOTE(bits) */
    let lname: u32 = MACH_MSGH_BITS_LOCAL(bits);
    let remote: *mut ipc_object = (*kmsg).ikm_header.msgh_remote_port as *mut ipc_object;
    let local: *mut ipc_object = (*kmsg).ikm_header.msgh_local_port as *mut ipc_object;

    /* translate the destination and reply ports */
    ipc_object_copyin_from_kernel(remote, rname);
    if io_valid(local) {
        ipc_object_copyin_from_kernel(local, lname);
    }

    /*
     *  The common case is a complex message with no reply port,
     *  because that is what the memory_object interface uses.
     */
    if bits == (MACH_MSGH_BITS_COMPLEX | MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0)) {
        bits = MACH_MSGH_BITS_COMPLEX | MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND, 0);
        (*kmsg).ikm_header.msgh_bits = bits;
    } else {
        bits = MACH_MSGH_BITS_OTHER(bits)
            | MACH_MSGH_BITS(ipc_object_copyin_type(rname), ipc_object_copyin_type(lname));
        (*kmsg).ikm_header.msgh_bits = bits;
        if (bits & MACH_MSGH_BITS_COMPLEX) == 0 {
            return;
        }
    }

    let mut saddr: vm_offset_t = addr_of_mut!((*kmsg).ikm_header).add(1) as vm_offset_t;
    let eaddr: vm_offset_t = (addr_of_mut!((*kmsg).ikm_header) as vm_offset_t)
        + (*kmsg).ikm_header.msgh_size as vm_offset_t;

    while saddr < eaddr {
        let typ_ptr = saddr as *mut mach_msg_type_long_t;
        let typ_short = saddr as *mut mach_msg_type_t;

        let is_inline = (*typ_short).msgt_inline() != 0;
        let longform = (*typ_short).msgt_longform() != 0;
        let name: u32;
        let size: u32;
        let number: u32;

        /* type->msgtl_header.msgt_deallocate not used */
        if longform {
            name = (*typ_ptr).msgtl_name as u32;
            size = (*typ_ptr).msgtl_size as u32;
            number = (*typ_ptr).msgtl_number as u32;
            saddr += core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(
                core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t
            ) {
                saddr = mach_msg_kernel_align(saddr);
            }
        } else {
            name = (*typ_short).msgt_name();
            size = (*typ_short).msgt_size();
            number = (*typ_short).msgt_number();
            saddr += core::mem::size_of::<mach_msg_type_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(core::mem::size_of::<mach_msg_type_t>() as vm_offset_t)
            {
                saddr = mach_msg_kernel_align(saddr);
            }
        }

        /* calculate length of data in bytes, rounding up */
        let _length: vm_size_t = (((number * size) + 7) >> 3) as vm_size_t;

        let is_port = MACH_MSG_TYPE_PORT_ANY(name);

        let data: vm_offset_t;
        if is_inline {
            data = saddr;
            saddr += _length;
        } else {
            /* The sender should supply ready-made memory for us. */
            data = *(saddr as *mut vm_offset_t);
            saddr += core::mem::size_of::<vm_offset_t>() as vm_offset_t;
        }

        if is_port {
            let newname: u32 = ipc_object_copyin_type(name);
            let objects: *mut *mut ipc_object = data as *mut *mut ipc_object;

            if longform {
                (*typ_ptr).msgtl_name = newname as u16;
            } else {
                (*typ_short).set_msgt_name(newname);
            }
            let mut i: u32 = 0;
            while i < number {
                let object = *objects.add(i as usize);
                if !io_valid(object) {
                    i += 1;
                    continue;
                }

                ipc_object_copyin_from_kernel(object, name);

                if newname == MACH_MSG_TYPE_PORT_RECEIVE
                    && ipc_port_check_circularity(
                        object as crate::mach_types::ipc_port_t,
                        remote as crate::mach_types::ipc_port_t,
                    ) != 0
                {
                    (*kmsg).ikm_header.msgh_bits |= MACH_MSGH_BITS_CIRCULAR;
                }
                i += 1;
            }
        }
        saddr = mach_msg_kernel_align(saddr);
    }
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyin_header — copyin port rights in the message header.
// ---------------------------------------------------------------------------

/// Inline mirror of `ipc_entry_lookup_failed` from `ipc/ipc_space.h`
/// (a debug printf when a user-supplied name is bogus).
#[inline]
unsafe fn ipc_entry_lookup_failed(
    msg: *mut crate::mach_types::mach_msg_header_t,
    port_name: mach_port_name_t,
) {
    if mach_port_name_valid(port_name) {
        let task = (*current_thread()).task as *mut u8;
        let name_ptr = task.add(crate::mach_types::OFFSETOF_TASK_NAME) as *const u8;
        crate::extern_c::printf(
            b"task %.*s looked up a bogus port %lu for %d, most probably a bug.\n\0".as_ptr()
                as *const _,
            crate::mach_types::TASK_NAME_SIZE as core::ffi::c_int,
            name_ptr,
            port_name as core::ffi::c_ulong,
            (*msg).msgh_id as core::ffi::c_int,
        );
        if crate::mach_port::mach_port_deallocate_debug != 0 {
            crate::extern_c::SoftDebugger(b"ipc_entry_lookup\0".as_ptr() as *const _);
        }
    }
}

#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}

#[inline]
unsafe fn ipc_entry_lookup_local(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> crate::mach_types::ipc_entry_t {
    let entry = crate::extern_c::rdxtree_lookup_common(
        core::ptr::addr_of!((*space).is_map),
        name as crate::mach_types::rdxtree_key_t,
        0,
    ) as crate::mach_types::ipc_entry_t;
    if entry.is_null() || ((*entry).ie_bits & IE_BITS_TYPE_MASK) == 0 {
        core::ptr::null_mut()
    } else {
        entry
    }
}

#[inline]
unsafe fn ipc_entry_dealloc(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: crate::mach_types::ipc_entry_t,
) {
    if ((*space).is_free_list_size as usize) < crate::mach_types::IS_FREE_LIST_SIZE_LIMIT {
        (*space).is_free_list_size += 1;
        (*entry).ie_bits = 0;
        (*entry).index.next_free = (*space).is_free_list;
        (*space).is_free_list = entry;
    } else {
        crate::extern_c::rdxtree_remove(
            addr_of_mut!((*space).is_map),
            name as crate::mach_types::rdxtree_key_t,
        );
        crate::extern_c::kmem_cache_free(
            addr_of_mut!(crate::ipc_entry::ipc_entry_cache),
            entry as crate::mach_types::vm_offset_t,
        );
    }
    (*space).is_size -= 1;
}

#[inline]
unsafe fn ipc_port_reference(port: crate::mach_types::ipc_port_t) {
    (*port).ip_target.ipt_object.io_references += 1;
}

#[inline]
unsafe fn ipc_port_release(port: crate::mach_types::ipc_port_t) {
    crate::ipc_object::ipc_object_release(addr_of_mut!((*port).ip_target.ipt_object));
}

/// `IP_TIMESTAMP_ORDER(one, two)` — one happened before two.
#[inline]
fn ip_timestamp_order(
    one: crate::mach_types::ipc_port_timestamp_t,
    two: crate::mach_types::ipc_port_timestamp_t,
) -> bool {
    ((one as i32).wrapping_sub(two as i32)) < 0
}

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyin_header(
    msg: *mut crate::mach_types::mach_msg_header_t,
    space: ipc_space_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    let mbits: u32 = (*msg).msgh_bits & !MACH_MSGH_BITS_CIRCULAR;

    let dest_name: mach_port_name_t = (*msg).msgh_remote_port;
    let reply_name: mach_port_name_t = (*msg).msgh_local_port;
    let mut kr: crate::mach_types::kern_return_t;

    /* first check for common cases */
    if notify == crate::mach_types::MACH_PORT_NAME_NULL {
        let ports = MACH_MSGH_BITS_PORTS(mbits);

        // case (COPY_SEND, 0): asynchronous send
        if ports == MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) {
            'abort_async: {
                if reply_name != crate::mach_types::MACH_PORT_NAME_NULL {
                    break 'abort_async;
                }

                crate::extern_c::lock_read(addr_of_mut!((*space).is_lock_data));
                if (*space).is_active == 0 {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_async;
                }

                let entry = ipc_entry_lookup_local(space, dest_name);
                if entry.is_null() {
                    ipc_entry_lookup_failed(msg, dest_name);
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_async;
                }
                let bits = (*entry).ie_bits;

                if ie_bits_type(bits) != crate::mach_types::MACH_PORT_TYPE_SEND {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_async;
                }

                /* optimized ipc_right_copyin */
                crate::kassert!(
                    (bits & crate::mach_types::IE_BITS_UREFS_MASK) > 0,
                    "IE_BITS_UREFS(bits) > 0"
                );

                let dest_port = (*entry).ie_object as crate::mach_types::ipc_port_t;
                crate::kassert!(!dest_port.is_null(), "dest_port != IP_NULL");

                /* ip_lock(dest_port) — no-op */
                /* unlock space — atomicity is fine */
                crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));

                if !((*dest_port).ip_target.ipt_object.io_bits as i32) < 0 {
                    /* !ip_active */
                    break 'abort_async;
                }

                crate::kassert!((*dest_port).ip_srights > 0, "dest_port->ip_srights > 0");
                (*dest_port).ip_srights += 1;
                ipc_port_reference(dest_port);

                (*msg).msgh_bits =
                    MACH_MSGH_BITS_OTHER(mbits) | MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND, 0);
                (*msg).msgh_remote_port = dest_port as usize as u32;
                return MACH_MSG_SUCCESS;
            }
            /* fall through to slow path */
        }
        // case (COPY_SEND, MAKE_SEND_ONCE): request
        else if ports == MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE) {
            'abort_request: {
                crate::extern_c::lock_read(addr_of_mut!((*space).is_lock_data));
                if (*space).is_active == 0 {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_request;
                }

                let entry = ipc_entry_lookup_local(space, dest_name);
                if entry.is_null() {
                    ipc_entry_lookup_failed(msg, dest_name);
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_request;
                }
                let bits = (*entry).ie_bits;

                if ie_bits_type(bits) != crate::mach_types::MACH_PORT_TYPE_SEND {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_request;
                }

                let dest_port = (*entry).ie_object as crate::mach_types::ipc_port_t;
                crate::kassert!(!dest_port.is_null(), "dest_port != IP_NULL");

                let entry2 = ipc_entry_lookup_local(space, reply_name);
                if entry2.is_null() {
                    ipc_entry_lookup_failed(msg, reply_name);
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_request;
                }
                let bits2 = (*entry2).ie_bits;

                if ie_bits_type(bits2) != crate::mach_types::MACH_PORT_TYPE_RECEIVE {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_request;
                }

                let reply_port = (*entry2).ie_object as crate::mach_types::ipc_port_t;
                crate::kassert!(!reply_port.is_null(), "reply_port != IP_NULL");

                /*
                 *  To do an atomic copyin, need simultaneous locks on
                 *  both ports and the space.  ip_lock_try is a no-op
                 *  always-succeed on NCPUS=1.
                 */
                /* ip_lock(dest_port) no-op */
                if !(((*dest_port).ip_target.ipt_object.io_bits as i32) < 0) {
                    /* !ip_active(dest) */
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_request;
                }
                /* ip_lock_try(reply_port) succeeds always */
                crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));

                crate::kassert!((*dest_port).ip_srights > 0, "dest_port->ip_srights > 0");
                (*dest_port).ip_srights += 1;
                ipc_port_reference(dest_port);

                crate::kassert!(
                    ((*reply_port).ip_target.ipt_object.io_bits as i32) < 0,
                    "ip_active(reply_port)"
                );
                crate::kassert!(
                    (*reply_port).ip_target.ipt_name == reply_name,
                    "reply_port->ip_receiver_name == reply_name"
                );
                crate::kassert!(
                    (*reply_port).data.receiver == space,
                    "reply_port->ip_receiver == space"
                );

                (*reply_port).ip_sorights += 1;
                ipc_port_reference(reply_port);

                (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
                    | MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND, MACH_MSG_TYPE_PORT_SEND_ONCE);
                (*msg).msgh_remote_port = dest_port as usize as u32;
                (*msg).msgh_local_port = reply_port as usize as u32;
                return MACH_MSG_SUCCESS;
            }
        }
        // case (MOVE_SEND_ONCE, 0): reply
        else if ports == MACH_MSGH_BITS(MACH_MSG_TYPE_MOVE_SEND_ONCE, 0) {
            'abort_reply: {
                if reply_name != crate::mach_types::MACH_PORT_NAME_NULL {
                    break 'abort_reply;
                }

                crate::extern_c::lock_write(addr_of_mut!((*space).is_lock_data));
                if (*space).is_active == 0 {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_reply;
                }

                let entry = ipc_entry_lookup_local(space, dest_name);
                if entry.is_null() {
                    ipc_entry_lookup_failed(msg, dest_name);
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_reply;
                }
                let bits = (*entry).ie_bits;

                if ie_bits_type(bits) != MACH_PORT_TYPE_SEND_ONCE {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_reply;
                }

                /* optimized ipc_right_copyin */
                crate::kassert!(
                    ie_bits_type(bits) == MACH_PORT_TYPE_SEND_ONCE,
                    "IE_BITS_TYPE(bits) == MACH_PORT_TYPE_SEND_ONCE"
                );
                crate::kassert!(
                    (bits & crate::mach_types::IE_BITS_UREFS_MASK) == 1,
                    "IE_BITS_UREFS(bits) == 1"
                );
                crate::kassert!(
                    (bits & IE_BITS_MAREQUEST) == 0,
                    "(bits & IE_BITS_MAREQUEST) == 0"
                );

                if (*entry).index.request != 0 {
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_reply;
                }

                let dest_port = (*entry).ie_object as crate::mach_types::ipc_port_t;
                crate::kassert!(!dest_port.is_null(), "dest_port != IP_NULL");

                if !(((*dest_port).ip_target.ipt_object.io_bits as i32) < 0) {
                    /* !ip_active */
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    break 'abort_reply;
                }

                crate::kassert!((*dest_port).ip_sorights > 0, "dest_port->ip_sorights > 0");

                (*entry).ie_object = core::ptr::null_mut();
                ipc_entry_dealloc(space, dest_name, entry);
                crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));

                (*msg).msgh_bits =
                    MACH_MSGH_BITS_OTHER(mbits) | MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
                (*msg).msgh_remote_port = dest_port as usize as u32;
                return MACH_MSG_SUCCESS;
            }
        }
        /* default: don't bother optimising — fall through */
    }

    /* slow path */
    let mut dest_type = mbits & 0xff;
    let mut reply_type = MACH_MSGH_BITS_LOCAL(mbits);

    if !MACH_MSG_TYPE_PORT_ANY_SEND(dest_type) {
        return MACH_SEND_INVALID_HEADER;
    }

    if (reply_type == 0 && reply_name != crate::mach_types::MACH_PORT_NAME_NULL)
        || (reply_type != 0 && !MACH_MSG_TYPE_PORT_ANY_SEND(reply_type))
    {
        return MACH_SEND_INVALID_HEADER;
    }

    crate::extern_c::lock_write(addr_of_mut!((*space).is_lock_data));

    let mut dest_port: *mut ipc_object = core::ptr::null_mut();
    let mut reply_port: *mut ipc_object = core::ptr::null_mut();
    let mut dest_soright: crate::mach_types::ipc_port_t = core::ptr::null_mut();
    let mut reply_soright: crate::mach_types::ipc_port_t = core::ptr::null_mut();
    let mut notify_port: crate::mach_types::ipc_port_t = core::ptr::null_mut();

    let outcome: u32 = 'main: {
        if (*space).is_active == 0 {
            break 'main 1; /* invalid_dest */
        }

        if notify != crate::mach_types::MACH_PORT_NAME_NULL {
            let entry = ipc_entry_lookup_local(space, notify);
            if entry.is_null()
                || ((*entry).ie_bits & crate::mach_types::MACH_PORT_TYPE_RECEIVE) == 0
            {
                if entry.is_null() {
                    ipc_entry_lookup_failed(msg, notify);
                }
                crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                return MACH_SEND_INVALID_NOTIFY;
            }
            notify_port = (*entry).ie_object as crate::mach_types::ipc_port_t;
        }

        if dest_name == reply_name {
            let entry = ipc_entry_lookup_local(space, dest_name);
            if entry.is_null() {
                ipc_entry_lookup_failed(msg, dest_name);
                break 'main 1;
            }

            crate::kassert!(reply_type != 0, "reply_type != 0");

            if ipc_right_copyin_check(space, dest_name, entry, reply_type) == 0 {
                break 'main 2; /* invalid_reply */
            }

            if dest_type == MACH_MSG_TYPE_MOVE_SEND_ONCE
                || reply_type == MACH_MSG_TYPE_MOVE_SEND_ONCE
            {
                break 'main 1;
            } else if dest_type == MACH_MSG_TYPE_MAKE_SEND
                || dest_type == MACH_MSG_TYPE_MAKE_SEND_ONCE
                || reply_type == MACH_MSG_TYPE_MAKE_SEND
                || reply_type == MACH_MSG_TYPE_MAKE_SEND_ONCE
            {
                kr = ipc_right_copyin(
                    space,
                    dest_name,
                    entry,
                    dest_type,
                    0,
                    &mut dest_port,
                    &mut dest_soright,
                );
                if kr != KERN_SUCCESS {
                    break 'main 1;
                }
                kr = ipc_right_copyin(
                    space,
                    dest_name,
                    entry,
                    reply_type,
                    1,
                    &mut reply_port,
                    &mut reply_soright,
                );
                let _ = kr;
            } else if dest_type == MACH_MSG_TYPE_COPY_SEND && reply_type == MACH_MSG_TYPE_COPY_SEND
            {
                kr = ipc_right_copyin(
                    space,
                    dest_name,
                    entry,
                    dest_type,
                    0,
                    &mut dest_port,
                    &mut dest_soright,
                );
                if kr != KERN_SUCCESS {
                    break 'main 1;
                }
                reply_port = ipc_port_copy_send(dest_port as crate::mach_types::ipc_port_t)
                    as *mut ipc_object;
                reply_soright = core::ptr::null_mut();
            } else if dest_type == MACH_MSG_TYPE_MOVE_SEND && reply_type == MACH_MSG_TYPE_MOVE_SEND
            {
                kr = ipc_right_copyin_two(
                    space,
                    dest_name,
                    entry,
                    &mut dest_port,
                    &mut dest_soright,
                );
                if kr != KERN_SUCCESS {
                    break 'main 1;
                }
                if ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE {
                    ipc_entry_dealloc(space, dest_name, entry);
                }
                reply_port = dest_port;
                reply_soright = core::ptr::null_mut();
            } else {
                /* (COPY_SEND, MOVE_SEND) or (MOVE_SEND, COPY_SEND) */
                let mut soright: crate::mach_types::ipc_port_t = core::ptr::null_mut();

                kr = ipc_right_copyin(
                    space,
                    dest_name,
                    entry,
                    MACH_MSG_TYPE_MOVE_SEND,
                    0,
                    &mut dest_port,
                    &mut soright,
                );
                if kr != KERN_SUCCESS {
                    break 'main 1;
                }
                if ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE {
                    ipc_entry_dealloc(space, dest_name, entry);
                }
                reply_port = ipc_port_copy_send(dest_port as crate::mach_types::ipc_port_t)
                    as *mut ipc_object;

                if dest_type == MACH_MSG_TYPE_MOVE_SEND {
                    dest_soright = soright;
                    reply_soright = core::ptr::null_mut();
                } else {
                    dest_soright = core::ptr::null_mut();
                    reply_soright = soright;
                }
            }
        } else if !mach_port_name_valid(reply_name) {
            let entry = ipc_entry_lookup_local(space, dest_name);
            if entry.is_null() {
                ipc_entry_lookup_failed(msg, dest_name);
                break 'main 1;
            }
            kr = ipc_right_copyin(
                space,
                dest_name,
                entry,
                dest_type,
                0,
                &mut dest_port,
                &mut dest_soright,
            );
            if kr != KERN_SUCCESS {
                break 'main 1;
            }
            if ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE {
                ipc_entry_dealloc(space, dest_name, entry);
            }
            reply_port = invalid_name_to_port(reply_name);
            reply_soright = core::ptr::null_mut();
        } else {
            /* general case: dest_name != reply_name, both valid */
            let dest_entry = ipc_entry_lookup_local(space, dest_name);
            if dest_entry.is_null() {
                ipc_entry_lookup_failed(msg, dest_name);
                break 'main 1;
            }
            let reply_entry = ipc_entry_lookup_local(space, reply_name);
            if reply_entry.is_null() {
                ipc_entry_lookup_failed(msg, reply_name);
                break 'main 2;
            }

            crate::kassert!(dest_entry != reply_entry, "dest_entry != reply_entry");
            crate::kassert!(reply_type != 0, "reply_type != 0");

            if ipc_right_copyin_check(space, reply_name, reply_entry, reply_type) == 0 {
                break 'main 2;
            }

            kr = ipc_right_copyin(
                space,
                dest_name,
                dest_entry,
                dest_type,
                0,
                &mut dest_port,
                &mut dest_soright,
            );
            if kr != KERN_SUCCESS {
                break 'main 1;
            }

            let saved_reply = (*reply_entry).ie_object as crate::mach_types::ipc_port_t;
            if !saved_reply.is_null() {
                ipc_port_reference(saved_reply);
            }

            kr = ipc_right_copyin(
                space,
                reply_name,
                reply_entry,
                reply_type,
                1,
                &mut reply_port,
                &mut reply_soright,
            );
            let _ = kr;

            if !saved_reply.is_null() && reply_port == IO_DEAD {
                let dest_p = dest_port as crate::mach_types::ipc_port_t;
                /* ip_lock(saved_reply) — no-op */
                let timestamp = (*saved_reply).data.timestamp;
                /* ip_lock(dest_p) — no-op */
                let must_undo = !(((*dest_p).ip_target.ipt_object.io_bits as i32) < 0)
                    && ip_timestamp_order((*dest_p).data.timestamp, timestamp);

                if must_undo {
                    ipc_right_copyin_undo(
                        space,
                        dest_name,
                        dest_entry,
                        dest_type,
                        dest_port,
                        dest_soright,
                    );
                    ipc_right_copyin_undo(
                        space,
                        reply_name,
                        reply_entry,
                        reply_type,
                        reply_port,
                        reply_soright,
                    );
                    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
                    if !dest_soright.is_null() {
                        ipc_notify_dead_name(dest_soright, dest_name);
                    }
                    crate::kassert!(reply_soright.is_null(), "reply_soright == IP_NULL");
                    ipc_port_release(saved_reply);
                    return MACH_SEND_INVALID_DEST;
                }
            }

            if ie_bits_type((*reply_entry).ie_bits) == MACH_PORT_TYPE_NONE {
                ipc_entry_dealloc(space, reply_name, reply_entry);
            }
            if ie_bits_type((*dest_entry).ie_bits) == MACH_PORT_TYPE_NONE {
                ipc_entry_dealloc(space, dest_name, dest_entry);
            }
            if !saved_reply.is_null() {
                ipc_port_release(saved_reply);
            }
        }

        0 /* success */
    };

    if outcome == 1 {
        crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
        return MACH_SEND_INVALID_DEST;
    } else if outcome == 2 {
        crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
        return MACH_SEND_INVALID_REPLY;
    }

    if notify != crate::mach_types::MACH_PORT_NAME_NULL && dest_soright == notify_port {
        ipc_port_release_sonce(dest_soright);
        dest_soright = core::ptr::null_mut();
    }

    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));

    if !dest_soright.is_null() {
        ipc_notify_port_deleted(dest_soright, dest_name);
    }
    if !reply_soright.is_null() {
        ipc_notify_port_deleted(reply_soright, reply_name);
    }

    dest_type = ipc_object_copyin_type(dest_type);
    reply_type = ipc_object_copyin_type(reply_type);

    (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits) | MACH_MSGH_BITS(dest_type, reply_type);
    (*msg).msgh_remote_port = dest_port as usize as u32;
    (*msg).msgh_local_port = reply_port as usize as u32;

    let _ = MACH_PORT_TYPE_DEAD_NAME;
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyin_body — copyin port rights + OOL memory in the body.
// ---------------------------------------------------------------------------

#[inline]
fn ipc_kobject_vm_page_list(ikot: u32) -> bool {
    ikot == IKOT_PAGING_REQUEST || ikot == IKOT_DEVICE || ikot == IKOT_USER_DEVICE
}

#[inline]
fn ipc_kobject_vm_page_steal(ikot: u32) -> bool {
    ikot == IKOT_PAGING_REQUEST
}

#[inline]
unsafe fn ip_kotype_of(port: crate::mach_types::ipc_port_t) -> u32 {
    (*port).ip_target.ipt_object.io_bits & crate::mach_types::IO_BITS_KOTYPE
}

/// `MACH_PORT_NAME_VALID(name) = name != 0 && name != ~0`.
#[inline]
fn mach_port_name_valid(name: mach_port_name_t) -> bool {
    name != 0 && name != !0u32
}

/// `invalid_name_to_port(name)` from `ipc/port.h`.
#[inline]
unsafe fn invalid_name_to_port(name: mach_port_name_t) -> *mut ipc_object {
    if name == 0 {
        core::ptr::null_mut()
    } else if name == !0u32 {
        !0usize as *mut ipc_object
    } else {
        crate::kpanic!("invalid_name_to_port() called with a valid name");
    }
}

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyin_body(
    kmsg: *mut ipc_kmsg_full,
    space: ipc_space_t,
    map: vm_map_t,
) -> mach_msg_return_t {
    let dest: *mut ipc_object = (*kmsg).ikm_header.msgh_remote_port as *mut ipc_object;
    let mut complex: bool = false;
    let dest_kotype = ip_kotype_of(dest as crate::mach_types::ipc_port_t);
    let use_page_lists = ipc_kobject_vm_page_list(dest_kotype);
    let steal_pages = ipc_kobject_vm_page_steal(dest_kotype);

    let mut saddr: vm_offset_t = addr_of_mut!((*kmsg).ikm_header).add(1) as vm_offset_t;
    let eaddr: vm_offset_t = (addr_of_mut!((*kmsg).ikm_header) as vm_offset_t)
        + (*kmsg).ikm_header.msgh_size as vm_offset_t;

    'outer: while saddr < eaddr {
        let taddr: vm_offset_t = saddr;
        let typ_ptr = saddr as *mut mach_msg_type_long_t;
        let typ_short = saddr as *mut mach_msg_type_t;

        let longform_first = (*typ_short).msgt_longform() != 0;
        if (eaddr - saddr) < core::mem::size_of::<mach_msg_type_t>() as vm_offset_t
            || (longform_first
                && (eaddr - saddr) < core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t)
        {
            ipc_kmsg_clean_partial(kmsg, taddr, 0, 0);
            return MACH_SEND_MSG_TOO_SMALL;
        }

        let is_inline = (*typ_short).msgt_inline() != 0;
        let dealloc = (*typ_short).msgt_deallocate() != 0;
        let longform = longform_first;
        let name: u32;
        let size: u32;
        let number: u32;

        if longform {
            name = (*typ_ptr).msgtl_name as u32;
            size = (*typ_ptr).msgtl_size as u32;
            number = (*typ_ptr).msgtl_number as u32;
            saddr += core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(
                core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t
            ) {
                saddr = mach_msg_kernel_align(saddr);
            }
        } else {
            name = (*typ_short).msgt_name();
            size = (*typ_short).msgt_size();
            number = (*typ_short).msgt_number();
            saddr += core::mem::size_of::<mach_msg_type_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(core::mem::size_of::<mach_msg_type_t>() as vm_offset_t)
            {
                saddr = mach_msg_kernel_align(saddr);
            }
        }

        let is_port = MACH_MSG_TYPE_PORT_ANY(name);

        if (is_port && !is_inline && size != PORT_NAME_T_SIZE_IN_BITS)
            || (is_port && is_inline && size != PORT_T_SIZE_IN_BITS)
            || (longform
                && ((*typ_ptr).msgtl_header.msgt_name() != 0
                    || (*typ_ptr).msgtl_header.msgt_size() != 0
                    || (*typ_ptr).msgtl_header.msgt_number() != 0))
            || (*typ_short).msgt_unused() != 0
            || (dealloc && is_inline)
        {
            ipc_kmsg_clean_partial(kmsg, taddr, 0, 0);
            return MACH_SEND_INVALID_TYPE;
        }

        /* calculate length of data in bytes, rounding up */
        let mut length: vm_size_t = (((number as u64) * (size as u64) + 7) >> 3) as vm_size_t;

        let data: vm_offset_t;
        if is_inline {
            let amount = length;
            if (eaddr - saddr) < amount {
                ipc_kmsg_clean_partial(kmsg, taddr, 0, 0);
                return MACH_SEND_MSG_TOO_SMALL;
            }
            data = saddr;
            saddr += amount;
        } else {
            if (eaddr - saddr) < core::mem::size_of::<vm_offset_t>() as vm_offset_t {
                ipc_kmsg_clean_partial(kmsg, taddr, 0, 0);
                return MACH_SEND_MSG_TOO_SMALL;
            }

            /* grab the out-of-line data */
            let addr: vm_offset_t = *(saddr as *mut vm_offset_t);

            'invalid_memory: {
                if is_port {
                    let user_length = length;
                    /* On i686, sizeof(mach_port_name_t) == sizeof(mach_port_t) == 4,
                     * so the size-mismatch path collapses to a no-op. */
                    if core::mem::size_of::<mach_port_name_t>()
                        != core::mem::size_of::<vm_offset_t>()
                    {
                        if longform {
                            (*typ_ptr).msgtl_size =
                                (core::mem::size_of::<vm_offset_t>() * 8) as u16;
                        } else {
                            (*typ_short)
                                .set_msgt_size((core::mem::size_of::<vm_offset_t>() * 8) as u32);
                        }
                        /* size unchanged here, kept for parity with C */
                        length =
                            (core::mem::size_of::<vm_offset_t>() * number as usize) as vm_size_t;
                    }

                    if length == 0 {
                        data = 0;
                    } else {
                        let d = kalloc(length);
                        if d == 0 {
                            ipc_kmsg_clean_partial(kmsg, taddr, 0, 0);
                            return MACH_SEND_INVALID_MEMORY;
                        }

                        if user_length != length {
                            /* size-mismatch path: copy port-by-port via copyin. */
                            let src = addr as *const mach_port_name_t;
                            let dst = d as *mut vm_offset_t;
                            let mut i: u32 = 0;
                            while i < number {
                                if copyin(
                                    src.add(i as usize) as *const core::ffi::c_void,
                                    dst.add(i as usize) as *mut core::ffi::c_void,
                                    core::mem::size_of::<mach_port_name_t>(),
                                ) != 0
                                {
                                    kfree(d, length);
                                    break 'invalid_memory;
                                }
                                i += 1;
                            }
                        } else if copyinmap(
                            map,
                            addr as *const core::ffi::c_char,
                            d as *mut core::ffi::c_char,
                            length as core::ffi::c_int,
                        ) != 0
                        {
                            kfree(d, length);
                            break 'invalid_memory;
                        }
                        if dealloc && vm_deallocate(map, addr, user_length) != KERN_SUCCESS {
                            kfree(d, length);
                            break 'invalid_memory;
                        }
                        data = d;
                    }
                } else if length == 0 {
                    data = 0;
                } else {
                    let mut copy: vm_map_copy_t = core::ptr::null_mut();
                    let kr = if use_page_lists {
                        vm_map_copyin_page_list(
                            map,
                            addr,
                            length,
                            dealloc as crate::mach_types::boolean_t,
                            steal_pages as crate::mach_types::boolean_t,
                            &mut copy,
                            0,
                        )
                    } else {
                        vm_map_copyin(
                            map,
                            addr,
                            length,
                            dealloc as crate::mach_types::boolean_t,
                            &mut copy,
                        )
                    };
                    if kr != KERN_SUCCESS {
                        break 'invalid_memory;
                    }
                    data = copy as vm_offset_t;
                }

                /* invalid_memory exit point follows the labeled-block */
                *(saddr as *mut vm_offset_t) = data;
                saddr += core::mem::size_of::<vm_offset_t>() as vm_offset_t;
                complex = true;
                /* Continue to the is_port port-extraction below. */
                if is_port {
                    let newname = ipc_object_copyin_type(name);
                    let objects = data as *mut *mut ipc_object;

                    if longform {
                        (*typ_ptr).msgtl_name = newname as u16;
                    } else {
                        (*typ_short).set_msgt_name(newname);
                    }

                    let mut i: u32 = 0;
                    while i < number {
                        let port = *(data as *mut mach_port_name_t).add(i as usize);
                        if !mach_port_name_valid(port) {
                            *objects.add(i as usize) = invalid_name_to_port(port);
                            i += 1;
                            continue;
                        }

                        let mut object: *mut ipc_object = core::ptr::null_mut();
                        let kr = ipc_object_copyin(space, port, name, &mut object);
                        if kr != KERN_SUCCESS {
                            ipc_kmsg_clean_partial(kmsg, taddr, 1, i);
                            return MACH_SEND_INVALID_RIGHT;
                        }

                        if newname == crate::mach_types::MACH_MSG_TYPE_PORT_RECEIVE
                            && ipc_port_check_circularity(
                                object as crate::mach_types::ipc_port_t,
                                dest as crate::mach_types::ipc_port_t,
                            ) != 0
                        {
                            (*kmsg).ikm_header.msgh_bits |= MACH_MSGH_BITS_CIRCULAR;
                        }

                        *objects.add(i as usize) = object;
                        i += 1;
                    }
                    complex = true;
                }
                saddr = mach_msg_kernel_align(saddr);
                continue 'outer;
            }

            /* invalid_memory: */
            ipc_kmsg_clean_partial(kmsg, taddr, 0, 0);
            return MACH_SEND_INVALID_MEMORY;
        }

        if is_port {
            let newname = ipc_object_copyin_type(name);
            let objects = data as *mut *mut ipc_object;

            if longform {
                (*typ_ptr).msgtl_name = newname as u16;
            } else {
                (*typ_short).set_msgt_name(newname);
            }

            let mut i: u32 = 0;
            while i < number {
                let port = *(data as *mut mach_port_name_t).add(i as usize);
                if !mach_port_name_valid(port) {
                    *objects.add(i as usize) = invalid_name_to_port(port);
                    i += 1;
                    continue;
                }

                let mut object: *mut ipc_object = core::ptr::null_mut();
                let kr = ipc_object_copyin(space, port, name, &mut object);
                if kr != KERN_SUCCESS {
                    ipc_kmsg_clean_partial(kmsg, taddr, 1, i);
                    return MACH_SEND_INVALID_RIGHT;
                }

                if newname == crate::mach_types::MACH_MSG_TYPE_PORT_RECEIVE
                    && ipc_port_check_circularity(
                        object as crate::mach_types::ipc_port_t,
                        dest as crate::mach_types::ipc_port_t,
                    ) != 0
                {
                    (*kmsg).ikm_header.msgh_bits |= MACH_MSGH_BITS_CIRCULAR;
                }

                *objects.add(i as usize) = object;
                i += 1;
            }
            complex = true;
        }
        saddr = mach_msg_kernel_align(saddr);
    }

    if !complex {
        (*kmsg).ikm_header.msgh_bits &= !MACH_MSGH_BITS_COMPLEX;
    }

    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyin — copyin port rights + out-of-line memory in the message.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyin(
    kmsg: *mut ipc_kmsg_full,
    space: ipc_space_t,
    map: vm_map_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    let mr = ipc_kmsg_copyin_header(addr_of_mut!((*kmsg).ikm_header), space, notify);
    if mr != MACH_MSG_SUCCESS {
        return mr;
    }

    if ((*kmsg).ikm_header.msgh_bits & MACH_MSGH_BITS_COMPLEX) == 0 {
        return MACH_MSG_SUCCESS;
    }

    ipc_kmsg_copyin_body(kmsg, space, map)
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_clean_partial — clean a partially-acquired kmsg.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_clean_partial(
    kmsg: *mut ipc_kmsg_full,
    mut eaddr: vm_offset_t,
    dolast: crate::mach_types::boolean_t,
    number: u32,
) {
    let mbits: u32 = (*kmsg).ikm_header.msgh_bits;

    crate::kassert!(
        (*kmsg).ikm_marequest == IMAR_NULL,
        "kmsg->ikm_marequest == IMAR_NULL"
    );

    let mut object: *mut ipc_object = (*kmsg).ikm_header.msgh_remote_port as *mut ipc_object;
    crate::kassert!(io_valid(object), "IO_VALID(remote)");
    ipc_object_destroy(object, mbits & 0xff /* MACH_MSGH_BITS_REMOTE */);

    object = (*kmsg).ikm_header.msgh_local_port as *mut ipc_object;
    if io_valid(object) {
        ipc_object_destroy(object, MACH_MSGH_BITS_LOCAL(mbits));
    }

    let saddr: vm_offset_t = addr_of_mut!((*kmsg).ikm_header).add(1) as vm_offset_t;
    ipc_kmsg_clean_body(saddr, eaddr);

    if dolast != 0 {
        let typ_ptr = eaddr as *mut mach_msg_type_long_t;
        let typ_short = eaddr as *mut mach_msg_type_t;

        let is_inline = (*typ_short).msgt_inline() != 0;
        let name: u32;
        let size: u32;
        let _rnumber: u32;

        if (*typ_short).msgt_longform() != 0 {
            name = (*typ_ptr).msgtl_name as u32;
            size = (*typ_ptr).msgtl_size as u32;
            _rnumber = (*typ_ptr).msgtl_number as u32;
            eaddr += core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(
                core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t
            ) {
                eaddr = mach_msg_kernel_align(eaddr);
            }
        } else {
            name = (*typ_short).msgt_name();
            size = (*typ_short).msgt_size();
            _rnumber = (*typ_short).msgt_number();
            eaddr += core::mem::size_of::<mach_msg_type_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(core::mem::size_of::<mach_msg_type_t>() as vm_offset_t)
            {
                eaddr = mach_msg_kernel_align(eaddr);
            }
        }

        /* calculate length of data in bytes, rounding up */
        let length: vm_size_t = (((_rnumber * size) + 7) >> 3) as vm_size_t;

        let is_port = MACH_MSG_TYPE_PORT_ANY(name);

        if is_port {
            let objects: *mut *mut ipc_object = if is_inline {
                eaddr as *mut *mut ipc_object
            } else {
                *(eaddr as *mut vm_offset_t) as *mut *mut ipc_object
            };

            /* destroy port rights carried in the message */
            let mut i: u32 = 0;
            while i < number {
                let obj = *objects.add(i as usize);
                if io_valid(obj) {
                    ipc_object_destroy(obj, name);
                }
                i += 1;
            }
        }

        if !is_inline {
            let data: vm_offset_t = *(eaddr as *mut vm_offset_t);

            /* destroy memory carried in the message */
            if length == 0 {
                crate::kassert!(data == 0, "data == 0");
            } else if is_port {
                kfree(data, length);
            } else {
                vm_map_copy_discard(data as vm_map_copy_t);
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_clean — release all rights/refs/memory held by the kmsg.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_clean(kmsg: *mut ipc_kmsg_full) {
    let mbits: u32 = (*kmsg).ikm_header.msgh_bits;

    let marequest = (*kmsg).ikm_marequest;
    if marequest != IMAR_NULL {
        ipc_marequest_destroy(marequest);
    }

    let mut object: *mut ipc_object = (*kmsg).ikm_header.msgh_remote_port as *mut ipc_object;
    if io_valid(object) {
        ipc_object_destroy(object, mbits & 0xff /* MACH_MSGH_BITS_REMOTE */);
    }

    object = (*kmsg).ikm_header.msgh_local_port as *mut ipc_object;
    if io_valid(object) {
        ipc_object_destroy(object, MACH_MSGH_BITS_LOCAL(mbits));
    }

    if (mbits & MACH_MSGH_BITS_COMPLEX) != 0 {
        let saddr: vm_offset_t = (addr_of_mut!((*kmsg).ikm_header).add(1)) as vm_offset_t;
        let eaddr: vm_offset_t = (addr_of_mut!((*kmsg).ikm_header) as vm_offset_t)
            + (*kmsg).ikm_header.msgh_size as vm_offset_t;

        ipc_kmsg_clean_body(saddr, eaddr);
    }
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_clean_body — body walker that destroys port rights / memory.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_clean_body(mut saddr: vm_offset_t, eaddr: vm_offset_t) {
    while saddr < eaddr {
        let typ_ptr = saddr as *mut mach_msg_type_long_t;
        let typ_short = saddr as *mut mach_msg_type_t;

        let is_inline = (*typ_short).msgt_inline() != 0;
        let name: u32;
        let size: u32;
        let number: u32;

        if (*typ_short).msgt_longform() != 0 {
            name = (*typ_ptr).msgtl_name as u32;
            size = (*typ_ptr).msgtl_size as u32;
            number = (*typ_ptr).msgtl_number as u32;
            saddr += core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(
                core::mem::size_of::<mach_msg_type_long_t>() as vm_offset_t
            ) {
                saddr = mach_msg_kernel_align(saddr);
            }
        } else {
            name = (*typ_short).msgt_name();
            size = (*typ_short).msgt_size();
            number = (*typ_short).msgt_number();
            saddr += core::mem::size_of::<mach_msg_type_t>() as vm_offset_t;
            if mach_msg_kernel_is_misaligned(core::mem::size_of::<mach_msg_type_t>() as vm_offset_t)
            {
                saddr = mach_msg_kernel_align(saddr);
            }
        }

        /* calculate length of data in bytes, rounding up */
        let length: vm_size_t = (((number * size) + 7) >> 3) as vm_size_t;

        let is_port = MACH_MSG_TYPE_PORT_ANY(name);

        if is_port {
            let objects: *mut *mut ipc_object;
            let mut n: u32 = number;
            if is_inline {
                objects = saddr as *mut *mut ipc_object;
                /* sanity check */
                while eaddr < (objects.add(n as usize) as vm_offset_t) {
                    n -= 1;
                }
            } else {
                objects = *(saddr as *mut vm_offset_t) as *mut *mut ipc_object;
            }

            /* destroy port rights carried in the message */
            let mut i: u32 = 0;
            while i < n {
                let object = *objects.add(i as usize);
                if io_valid(object) {
                    ipc_object_destroy(object, name);
                }
                i += 1;
            }
        }

        if is_inline {
            saddr += length;
        } else {
            let data: vm_offset_t = *(saddr as *mut vm_offset_t);

            /* destroy memory carried in the message */
            if length == 0 {
                crate::kassert!(data == 0, "data == 0");
            } else if is_port {
                kfree(data, length);
            } else {
                vm_map_copy_discard(data as vm_map_copy_t);
            }

            saddr += core::mem::size_of::<vm_offset_t>() as vm_offset_t;
        }
        saddr = mach_msg_kernel_align(saddr);
    }
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_put — copyoutmsg the kmsg to user space, free buffer.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_put(
    msg: *mut crate::mach_types::mach_msg_user_header_t,
    kmsg: *mut ipc_kmsg_full,
    size: crate::mach_types::mach_msg_size_t,
) -> mach_msg_return_t {
    /* ikm_check_initialized(kmsg, kmsg->ikm_size) */
    crate::kassert!(
        (*kmsg).ikm_size == (*kmsg).ikm_size,
        "ikm_check_initialized: ikm_size matches"
    );
    crate::kassert!(
        (*kmsg).ikm_marequest == IMAR_NULL,
        "ikm_check_initialized: ikm_marequest == IMAR_NULL"
    );

    let mr = if copyoutmsg(
        addr_of_mut!((*kmsg).ikm_header) as *const core::ffi::c_void,
        msg as *mut core::ffi::c_void,
        size as usize,
    ) != 0
    {
        MACH_RCV_INVALID_DATA
    } else {
        MACH_MSG_SUCCESS
    };

    /* ikm_cache_free(kmsg): if size matches the cache slot and the slot
     * is empty, store; otherwise ikm_free. */
    if (*kmsg).ikm_size == IKM_SAVED_KMSG_SIZE && ipc_kmsg_cache[0] == IKM_NULL {
        ipc_kmsg_cache[0] = kmsg;
    } else {
        kfree(kmsg as crate::mach_types::vm_offset_t, (*kmsg).ikm_size);
    }

    mr
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_get — allocate buffer + copyinmsg from user space.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_get(
    msg: *mut crate::mach_types::mach_msg_user_header_t,
    size: crate::mach_types::mach_msg_size_t,
    kmsgp: *mut *mut ipc_kmsg_full,
) -> mach_msg_return_t {
    let ksize: crate::mach_types::mach_msg_size_t = size * IKM_EXPAND_FACTOR;

    if (size as usize) < core::mem::size_of::<crate::mach_types::mach_msg_user_header_t>()
        || ((size as usize) & (MACH_MSG_USER_ALIGNMENT - 1)) != 0
    {
        return MACH_SEND_MSG_TOO_SMALL;
    }

    let kmsg: *mut ipc_kmsg_full;
    if ksize <= IKM_SAVED_MSG_SIZE {
        /* ikm_cache_alloc(): try ikm_cache_alloc_try first; on miss,
         * ikm_alloc(IKM_SAVED_MSG_SIZE) + ikm_init(...). */
        let cached = ipc_kmsg_cache[0];
        if cached != IKM_NULL {
            ipc_kmsg_cache[0] = IKM_NULL;
            crate::kassert!(
                (*cached).ikm_size == IKM_SAVED_KMSG_SIZE,
                "ikm_check_initialized: ikm_size == IKM_SAVED_KMSG_SIZE"
            );
            crate::kassert!(
                (*cached).ikm_marequest == IMAR_NULL,
                "ikm_check_initialized: ikm_marequest == IMAR_NULL"
            );
            kmsg = cached;
        } else {
            let total = IKM_SAVED_MSG_SIZE as crate::mach_types::vm_size_t + IKM_OVERHEAD;
            let k = kalloc(total) as *mut ipc_kmsg_full;
            if k == IKM_NULL {
                return MACH_SEND_NO_BUFFER;
            }
            (*k).ikm_size = total;
            (*k).ikm_marequest = IMAR_NULL;
            kmsg = k;
        }
    } else {
        let total = ksize as crate::mach_types::vm_size_t + IKM_OVERHEAD;
        let k = kalloc(total) as *mut ipc_kmsg_full;
        if k == IKM_NULL {
            return MACH_SEND_NO_BUFFER;
        }
        (*k).ikm_size = total;
        (*k).ikm_marequest = IMAR_NULL;
        kmsg = k;
    }

    if copyinmsg(
        msg as *const core::ffi::c_void,
        addr_of_mut!((*kmsg).ikm_header) as *mut core::ffi::c_void,
        size as usize,
        (*kmsg).ikm_size as usize,
    ) != 0
    {
        /* ikm_free(kmsg) */
        kfree(kmsg as crate::mach_types::vm_offset_t, (*kmsg).ikm_size);
        return MACH_SEND_INVALID_DATA;
    }

    *kmsgp = kmsg;
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_get_from_kernel — allocate buffer + copy a kernel-side message.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_get_from_kernel(
    msg: *mut crate::mach_types::mach_msg_header_t,
    size: crate::mach_types::mach_msg_size_t,
    kmsgp: *mut *mut ipc_kmsg_full,
) -> mach_msg_return_t {
    crate::kassert!(
        (size as usize) >= core::mem::size_of::<crate::mach_types::mach_msg_header_t>(),
        "size >= sizeof(mach_msg_header_t)"
    );
    crate::kassert!(
        ((size as usize) & (MACH_MSG_KERNEL_ALIGNMENT - 1)) == 0,
        "!mach_msg_kernel_is_misaligned(size)"
    );

    /* ikm_alloc(size) = (ipc_kmsg_t) kalloc(size + IKM_OVERHEAD). */
    let total = size as crate::mach_types::vm_size_t + IKM_OVERHEAD;
    let kmsg = kalloc(total) as *mut ipc_kmsg_full;
    if kmsg == IKM_NULL {
        return MACH_SEND_NO_BUFFER;
    }
    /* ikm_init: ikm_size = total; ikm_marequest = IMAR_NULL. */
    (*kmsg).ikm_size = total;
    (*kmsg).ikm_marequest = IMAR_NULL;

    core::ptr::copy_nonoverlapping(
        msg as *const u8,
        addr_of_mut!((*kmsg).ikm_header) as *mut u8,
        size as usize,
    );

    (*kmsg).ikm_header.msgh_size = size;
    *kmsgp = kmsg;
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_put_to_kernel — copy kmsg into a kernel buffer, then free.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_put_to_kernel(
    msg: *mut crate::mach_types::mach_msg_header_t,
    kmsg: *mut ipc_kmsg_full,
    size: crate::mach_types::mach_msg_size_t,
) {
    /* DIPC is not defined; assert(!KMSG_IN_DIPC(kmsg)) is dropped. */
    core::ptr::copy_nonoverlapping(
        addr_of_mut!((*kmsg).ikm_header) as *const u8,
        msg as *mut u8,
        size as usize,
    );
    /* ikm_free(kmsg) = kfree((vm_offset_t)kmsg, kmsg->ikm_size). */
    kfree(kmsg as crate::mach_types::vm_offset_t, (*kmsg).ikm_size);
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_free — free a kernel message buffer.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_free(kmsg: *mut ipc_kmsg_full) {
    kfree(kmsg as crate::mach_types::vm_offset_t, (*kmsg).ikm_size);
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_destroy — release all rights/refs/memory held by the kmsg.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_destroy(kmsg: *mut ipc_kmsg_full) {
    /*
     *  ipc_kmsg_clean can cause more messages to be destroyed.  Curtail
     *  recursion by queueing messages.  If a message is already queued,
     *  then this is a recursive call.
     */
    let queue: *mut ipc_kmsg_queue = addr_of_mut!((*current_thread()).ith_messages);
    let empty = (*queue).ikmq_base.is_null();
    ipc_kmsg_enqueue(queue, kmsg);

    if empty {
        /* must leave kmsg in queue while cleaning it */
        loop {
            let k = (*queue).ikmq_base as *mut ipc_kmsg_full;
            if k == IKM_NULL {
                break;
            }
            ipc_kmsg_clean(k);
            ipc_kmsg_rmqueue(queue, k);
            /* ikm_free(k) = kfree((vm_offset_t) k, k->ikm_size) */
            kfree(k as crate::mach_types::vm_offset_t, (*k).ikm_size);
        }
    }
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_queue_next — kmsg following the given one, or IKM_NULL.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_queue_next(
    queue: *mut ipc_kmsg_queue,
    kmsg: *mut ipc_kmsg_full,
) -> *mut ipc_kmsg_full {
    crate::kassert!(
        !(*queue).ikmq_base.is_null(),
        "queue->ikmq_base != IKM_NULL"
    );

    let mut next: *mut ipc_kmsg_full = (*kmsg).ikm_next;
    if (*queue).ikmq_base as *mut ipc_kmsg_full == next {
        next = IKM_NULL;
    }
    next
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_rmqueue — pull a kmsg out of a queue.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_rmqueue(queue: *mut ipc_kmsg_queue, kmsg: *mut ipc_kmsg_full) {
    crate::kassert!(
        !(*queue).ikmq_base.is_null(),
        "queue->ikmq_base != IKM_NULL"
    );

    let next: *mut ipc_kmsg_full = (*kmsg).ikm_next;
    let prev: *mut ipc_kmsg_full = (*kmsg).ikm_prev;

    if next == kmsg {
        crate::kassert!(prev == kmsg, "prev == kmsg");
        crate::kassert!(
            (*queue).ikmq_base as *mut ipc_kmsg_full == kmsg,
            "queue->ikmq_base == kmsg"
        );

        (*queue).ikmq_base = core::ptr::null_mut();
    } else {
        if (*queue).ikmq_base as *mut ipc_kmsg_full == kmsg {
            (*queue).ikmq_base = next as *mut crate::mach_types::ipc_kmsg;
        }

        (*next).ikm_prev = prev;
        (*prev).ikm_next = next;
    }
    /* ikm_mark_bogus is empty */
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_dequeue — expands the C `ipc_kmsg_rmqueue_first_macro` inline.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_dequeue(queue: *mut ipc_kmsg_queue) -> *mut ipc_kmsg_full {
    let first: *mut ipc_kmsg_full = (*queue).ikmq_base as *mut ipc_kmsg_full;

    if first != IKM_NULL {
        /* assert((queue)->ikmq_base == kmsg); */
        crate::kassert!(
            (*queue).ikmq_base as *mut ipc_kmsg_full == first,
            "queue->ikmq_base == first"
        );

        let next: *mut ipc_kmsg_full = (*first).ikm_next;
        if next == first {
            crate::kassert!((*first).ikm_prev == first, "first->ikm_prev == first");
            (*queue).ikmq_base = core::ptr::null_mut();
        } else {
            let prev: *mut ipc_kmsg_full = (*first).ikm_prev;
            (*queue).ikmq_base = next as *mut crate::mach_types::ipc_kmsg;
            (*next).ikm_prev = prev;
            (*prev).ikm_next = next;
        }
        /* ikm_mark_bogus is empty */
    }

    first
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_enqueue — expands the C `ipc_kmsg_enqueue_macro` inline.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_enqueue(queue: *mut ipc_kmsg_queue, kmsg: *mut ipc_kmsg_full) {
    let first: *mut ipc_kmsg_full = (*queue).ikmq_base as *mut ipc_kmsg_full;

    if first == IKM_NULL {
        (*queue).ikmq_base = kmsg as *mut crate::mach_types::ipc_kmsg;
        (*kmsg).ikm_next = kmsg;
        (*kmsg).ikm_prev = kmsg;
    } else {
        let last: *mut ipc_kmsg_full = (*first).ikm_prev;

        (*kmsg).ikm_next = first;
        (*kmsg).ikm_prev = last;
        (*first).ikm_prev = kmsg;
        (*last).ikm_next = kmsg;
    }
}

// ---------------------------------------------------------------------------
//  ipc_kmsg_copyout_header — copy out port rights in the message header.
// ---------------------------------------------------------------------------

/// `ipc_port_flag_protected_payload(port)` from `ipc/ipc_port.h`.
#[inline]
unsafe fn ipc_port_flag_protected_payload(port: crate::mach_types::ipc_port_t) -> bool {
    ((*port).ip_target.ipt_object.io_bits & IO_BITS_PROTECTED_PAYLOAD) != 0
}

/// `ipc_entry_get` static-inline from `ipc/ipc_space.h`.  Tries to allocate
/// an entry out of the space (must be write-locked and active).
#[inline]
unsafe fn ipc_entry_get(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
    entryp: *mut crate::mach_types::ipc_entry_t,
) -> crate::mach_types::kern_return_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");

    /* Get entry from the free list.  */
    let free_entry: crate::mach_types::ipc_entry_t = (*space).is_free_list;
    if free_entry.is_null() {
        return KERN_NO_SPACE;
    }

    (*space).is_free_list = (*free_entry).index.next_free;
    (*space).is_free_list_size -= 1;

    /*
     *  Initialize the new entry.  IE_BITS_GEN_MASK == 0 / IE_BITS_GEN_ONE == 0
     *  on i686, so the gen book-keeping collapses to clearing ie_bits and
     *  ie_request.  MACH_PORT_MAKE(name, gen) == name.
     */
    crate::kassert!(
        ((*free_entry).ie_bits & !0u32/* IE_BITS_GEN_MASK == 0 */) == (*free_entry).ie_bits,
        "(free_entry->ie_bits &~ IE_BITS_GEN_MASK) == 0"
    );
    let new_name: mach_port_name_t = (*free_entry).ie_name;
    (*free_entry).ie_bits = 0;
    (*free_entry).index.request = 0;

    crate::kassert!(
        mach_port_name_valid(new_name),
        "MACH_PORT_NAME_VALID(new_name)"
    );
    crate::kassert!(
        (*free_entry).ie_object.is_null(),
        "free_entry->ie_object == IO_NULL"
    );

    (*space).is_size += 1;
    *namep = new_name;
    *entryp = free_entry;
    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn ipc_kmsg_copyout_header(
    msg: *mut crate::mach_types::mach_msg_header_t,
    space: ipc_space_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    let mbits: u32 = (*msg).msgh_bits;
    let dest: crate::mach_types::ipc_port_t =
        (*msg).msgh_remote_port as crate::mach_types::ipc_port_t;

    crate::kassert!(io_valid(dest as *mut ipc_object), "IP_VALID(dest)");

    /* first check for common cases */

    if notify == crate::mach_types::MACH_PORT_NULL {
        let ports = MACH_MSGH_BITS_PORTS(mbits);

        // (MACH_MSG_TYPE_PORT_SEND, 0): receiving an asynchronous message
        if ports == MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND, 0) {
            'fast_async: {
                ip_lock_noop(dest);
                if !ip_active(dest) {
                    ip_unlock_noop(dest);
                    break 'fast_async;
                }

                /* optimized ipc_object_copyout_dest */
                crate::kassert!((*dest).ip_srights > 0, "dest->ip_srights > 0");
                ip_release(dest);

                let dest_name: mach_port_name_t = if (*dest).data.receiver == space {
                    (*dest).ip_target.ipt_name
                } else {
                    crate::mach_types::MACH_PORT_NULL
                };
                let payload: u32 = (*dest).ip_protected_payload;

                (*dest).ip_srights -= 1;
                if (*dest).ip_srights == 0 {
                    let nsrequest = (*dest).ip_nsrequest;
                    if !nsrequest.is_null() {
                        let mscount = (*dest).ip_mscount;
                        (*dest).ip_nsrequest = IP_NULL;
                        ip_unlock_noop(dest);
                        ipc_notify_no_senders(nsrequest, mscount);
                    } else {
                        ip_unlock_noop(dest);
                    }
                } else {
                    ip_unlock_noop(dest);
                }

                if !ipc_port_flag_protected_payload(dest) {
                    (*msg).msgh_bits =
                        MACH_MSGH_BITS_OTHER(mbits) | MACH_MSGH_BITS(0, MACH_MSG_TYPE_PORT_SEND);
                    (*msg).msgh_local_port = dest_name;
                } else {
                    (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
                        | MACH_MSGH_BITS(0, MACH_MSG_TYPE_PROTECTED_PAYLOAD);
                    (*msg).msgh_local_port = payload; /* msgh_protected_payload (union) */
                }
                (*msg).msgh_remote_port = crate::mach_types::MACH_PORT_NULL;
                return MACH_MSG_SUCCESS;
            }
        }

        // (MACH_MSG_TYPE_PORT_SEND, MACH_MSG_TYPE_PORT_SEND_ONCE):
        // receiving a request message
        if ports == MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND, MACH_MSG_TYPE_PORT_SEND_ONCE) {
            'fast_request: {
                let reply: crate::mach_types::ipc_port_t =
                    (*msg).msgh_local_port as crate::mach_types::ipc_port_t;

                if !io_valid(reply as *mut ipc_object) {
                    break 'fast_request;
                }

                is_write_lock(space);
                if (*space).is_active == 0 || (*space).is_free_list.is_null() {
                    is_write_unlock(space);
                    break 'fast_request;
                }

                /* simultaneous locks on both ports and the space.  With NCPUS == 1
                 * the locks are no-ops; ip_lock_try always succeeds. */
                ip_lock_noop(dest);
                if !ip_active(dest) {
                    ip_unlock_noop(dest);
                    is_write_unlock(space);
                    break 'fast_request;
                }

                if !ip_active(reply) {
                    ip_unlock_noop(reply);
                    ip_unlock_noop(dest);
                    is_write_unlock(space);
                    break 'fast_request;
                }

                crate::kassert!((*reply).ip_sorights > 0, "reply->ip_sorights > 0");
                ip_unlock_noop(reply);

                let mut reply_name: mach_port_name_t = 0;
                let mut entry: crate::mach_types::ipc_entry_t = core::ptr::null_mut();
                let kr = ipc_entry_get(space, &mut reply_name, &mut entry);
                if kr != 0 {
                    ip_unlock_noop(reply);
                    ip_unlock_noop(dest);
                    is_write_unlock(space);
                    break 'fast_request;
                }

                {
                    /* IE_BITS_GEN_MASK == 0 / IE_BITS_GEN_ONE == 0 on i686 */
                    crate::kassert!(
                        ((*entry).ie_bits & !0u32/* IE_BITS_GEN_MASK */) == (*entry).ie_bits,
                        "(entry->ie_bits &~ IE_BITS_GEN_MASK) == 0"
                    );
                    let gen: u32 = (*entry).ie_bits + 0 /* IE_BITS_GEN_ONE */;
                    /* optimized ipc_right_copyout */
                    (*entry).ie_bits = gen | (MACH_PORT_TYPE_SEND_ONCE | 1);
                }

                crate::kassert!(
                    mach_port_name_valid(reply_name),
                    "MACH_PORT_NAME_VALID(reply_name)"
                );
                (*entry).ie_object = reply as *mut ipc_object;
                is_write_unlock(space);

                /* optimized ipc_object_copyout_dest */
                crate::kassert!((*dest).ip_srights > 0, "dest->ip_srights > 0");
                ip_release(dest);

                let dest_name: mach_port_name_t = if (*dest).data.receiver == space {
                    (*dest).ip_target.ipt_name
                } else {
                    crate::mach_types::MACH_PORT_NULL
                };
                let payload: u32 = (*dest).ip_protected_payload;

                (*dest).ip_srights -= 1;
                if (*dest).ip_srights == 0 {
                    let nsrequest = (*dest).ip_nsrequest;
                    if !nsrequest.is_null() {
                        let mscount = (*dest).ip_mscount;
                        (*dest).ip_nsrequest = IP_NULL;
                        ip_unlock_noop(dest);
                        ipc_notify_no_senders(nsrequest, mscount);
                    } else {
                        ip_unlock_noop(dest);
                    }
                } else {
                    ip_unlock_noop(dest);
                }

                if !ipc_port_flag_protected_payload(dest) {
                    (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
                        | MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, MACH_MSG_TYPE_PORT_SEND);
                    (*msg).msgh_local_port = dest_name;
                } else {
                    (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
                        | MACH_MSGH_BITS(
                            MACH_MSG_TYPE_PORT_SEND_ONCE,
                            MACH_MSG_TYPE_PROTECTED_PAYLOAD,
                        );
                    (*msg).msgh_local_port = payload; /* union */
                }
                (*msg).msgh_remote_port = reply_name;
                return MACH_MSG_SUCCESS;
            }
        }

        // (MACH_MSG_TYPE_PORT_SEND_ONCE, 0): receiving a reply message
        if ports == MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0) {
            'fast_reply: {
                ip_lock_noop(dest);
                if !ip_active(dest) {
                    ip_unlock_noop(dest);
                    break 'fast_reply;
                }

                /* optimized ipc_object_copyout_dest */
                crate::kassert!((*dest).ip_sorights > 0, "dest->ip_sorights > 0");

                let payload: u32 = (*dest).ip_protected_payload;

                let dest_name: mach_port_name_t;
                if (*dest).data.receiver == space {
                    ip_release(dest);
                    (*dest).ip_sorights -= 1;
                    dest_name = (*dest).ip_target.ipt_name;
                    ip_unlock_noop(dest);
                } else {
                    ip_unlock_noop(dest);
                    ipc_notify_send_once(dest);
                    dest_name = crate::mach_types::MACH_PORT_NULL;
                }

                if !ipc_port_flag_protected_payload(dest) {
                    (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
                        | MACH_MSGH_BITS(0, MACH_MSG_TYPE_PORT_SEND_ONCE);
                    (*msg).msgh_local_port = dest_name;
                } else {
                    (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
                        | MACH_MSGH_BITS(0, MACH_MSG_TYPE_PROTECTED_PAYLOAD);
                    (*msg).msgh_local_port = payload; /* union */
                }
                (*msg).msgh_remote_port = crate::mach_types::MACH_PORT_NULL;
                return MACH_MSG_SUCCESS;
            }
        }
        /* default: don't bother optimizing — fall through to slow path */
    }

    /* slow path */
    let dest_type: u32 = mbits & 0xff; /* MACH_MSGH_BITS_REMOTE(mbits) */
    let reply_type: u32 = MACH_MSGH_BITS_LOCAL(mbits);
    let mut reply: crate::mach_types::ipc_port_t =
        (*msg).msgh_local_port as crate::mach_types::ipc_port_t;
    let dest_name: mach_port_name_t;
    let reply_name: mach_port_name_t;
    let payload: u32;

    let copyout_dest: bool;

    if io_valid(reply as *mut ipc_object) {
        let mut notify_port: crate::mach_types::ipc_port_t = IP_NULL;
        let mut entry: crate::mach_types::ipc_entry_t = core::ptr::null_mut();
        let mut local_reply_name: mach_port_name_t = 0;
        let mut goto_copyout_dest: bool = false;

        is_write_lock(space);

        let outcome: mach_msg_return_t = 'space_locked: loop {
            if (*space).is_active == 0 {
                is_write_unlock(space);
                break 'space_locked MACH_RCV_HEADER_ERROR | MACH_MSG_IPC_SPACE;
            }

            if notify != crate::mach_types::MACH_PORT_NULL {
                notify_port = ipc_port_lookup_notify(space, notify);
                if notify_port == IP_NULL {
                    is_write_unlock(space);
                    break 'space_locked MACH_RCV_INVALID_NOTIFY;
                }
            } else {
                notify_port = IP_NULL;
            }

            if reply_type != MACH_MSG_TYPE_PORT_SEND_ONCE
                && ipc_right_reverse(
                    space,
                    reply as *mut ipc_object,
                    &mut local_reply_name,
                    &mut entry,
                ) != 0
            {
                /* reply port is locked and active */
                crate::kassert!(
                    ((*entry).ie_bits & MACH_PORT_TYPE_SEND_RECEIVE) != 0,
                    "entry->ie_bits & MACH_PORT_TYPE_SEND_RECEIVE"
                );
                break 'space_locked 0; /* fall through to copyout */
            }

            ip_lock_noop(reply);
            if !ip_active(reply) {
                ip_release(reply);
                io_check_unlock(reply as *mut ipc_object);

                if notify_port != IP_NULL {
                    ipc_port_release_sonce(notify_port);
                }

                ip_lock_noop(dest);
                is_write_unlock(space);

                reply = !0usize as crate::mach_types::ipc_port_t; /* IP_DEAD */
                local_reply_name = MACH_PORT_NAME_DEAD;
                goto_copyout_dest = true;
                break 'space_locked 0;
            }

            let kr = ipc_entry_alloc(space, &mut local_reply_name, &mut entry);
            if kr != KERN_SUCCESS {
                ip_unlock_noop(reply);

                if notify_port != IP_NULL {
                    ipc_port_release_sonce(notify_port);
                }

                is_write_unlock(space);
                if kr == KERN_RESOURCE_SHORTAGE {
                    break 'space_locked MACH_RCV_HEADER_ERROR | MACH_MSG_IPC_KERNEL;
                } else {
                    break 'space_locked MACH_RCV_HEADER_ERROR | MACH_MSG_IPC_SPACE;
                }
            }

            crate::kassert!(
                ie_bits_type((*entry).ie_bits) == MACH_PORT_TYPE_NONE,
                "IE_BITS_TYPE(entry->ie_bits) == MACH_PORT_TYPE_NONE"
            );
            crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");

            if notify_port == IP_NULL {
                /* not making a dead-name request */
                (*entry).ie_object = reply as *mut ipc_object;
                break 'space_locked 0;
            }

            let mut request: crate::mach_types::ipc_port_request_index_t = 0;
            let kr2 = ipc_port_dnrequest(reply, local_reply_name, notify_port, &mut request);
            if kr2 != KERN_SUCCESS {
                ip_unlock_noop(reply);

                ipc_port_release_sonce(notify_port);

                ipc_entry_dealloc(space, local_reply_name, entry);
                is_write_unlock(space);

                ip_lock_noop(reply);
                if !ip_active(reply) {
                    /* will fail next time around loop */
                    ip_unlock_noop(reply);
                    is_write_lock(space);
                    continue;
                }

                let kr3 = ipc_port_dngrow(reply);
                /* port is unlocked */
                if kr3 != KERN_SUCCESS {
                    break 'space_locked MACH_RCV_HEADER_ERROR | MACH_MSG_IPC_KERNEL;
                }

                is_write_lock(space);
                continue;
            }

            notify_port = IP_NULL; /* don't release right below */

            (*entry).ie_object = reply as *mut ipc_object;
            (*entry).index.request = request;
            break 'space_locked 0;
        };

        if outcome != 0 {
            return outcome;
        }

        if goto_copyout_dest {
            reply_name = local_reply_name;
            copyout_dest = true;
        } else {
            /* space and reply port are locked and active */
            ipc_port_reference(reply); /* hold onto the reply port */

            let kr = ipc_right_copyout(
                space,
                local_reply_name,
                entry,
                reply_type,
                1, /* TRUE */
                reply as *mut ipc_object,
            );
            /* reply port is unlocked */
            crate::kassert!(kr == KERN_SUCCESS, "kr == KERN_SUCCESS");

            if notify_port != IP_NULL {
                ipc_port_release_sonce(notify_port);
            }

            ip_lock_noop(dest);
            is_write_unlock(space);

            reply_name = local_reply_name;
            copyout_dest = false;
        }
    } else {
        /*
         *  No reply port — easy case.
         */
        is_read_lock(space);
        if (*space).is_active == 0 {
            is_read_unlock(space);
            return MACH_RCV_HEADER_ERROR | MACH_MSG_IPC_SPACE;
        }

        if notify != crate::mach_types::MACH_PORT_NULL {
            /* must check notify even though it won't be used */
            let entry = ipc_entry_lookup_local(space, notify);
            if entry.is_null() || ((*entry).ie_bits & MACH_PORT_TYPE_RECEIVE) == 0 {
                if entry.is_null() {
                    ipc_entry_lookup_failed(msg, notify);
                }
                is_read_unlock(space);
                return MACH_RCV_INVALID_NOTIFY;
            }
        }

        ip_lock_noop(dest);
        is_read_unlock(space);

        reply_name = invalid_port_to_name((*msg).msgh_local_port as *mut ipc_object);
        copyout_dest = false;
    }

    /*
     *  copyout_dest:
     *  At this point, the space is unlocked and the destination port is
     *  locked.  reply_name is taken care of; we still need dest_name.
     */
    payload = (*dest).ip_protected_payload;

    if ip_active(dest) {
        let mut name: mach_port_name_t = 0;
        ipc_object_copyout_dest(space, dest as *mut ipc_object, dest_type, &mut name);
        /* dest is unlocked */
        dest_name = name;
    } else {
        let timestamp: crate::mach_types::ipc_port_timestamp_t = (*dest).data.timestamp;
        ip_release(dest);
        io_check_unlock(dest as *mut ipc_object);

        if io_valid(reply as *mut ipc_object) {
            ip_lock_noop(reply);
            if ip_active(reply) || ip_timestamp_order(timestamp, (*reply).data.timestamp) {
                dest_name = MACH_PORT_NAME_DEAD;
            } else {
                dest_name = MACH_PORT_NAME_NULL;
            }
            ip_unlock_noop(reply);
        } else {
            dest_name = MACH_PORT_NAME_DEAD;
        }
    }

    if io_valid(reply as *mut ipc_object) {
        ipc_port_release(reply);
    }

    if !ipc_port_flag_protected_payload(dest) {
        (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits) | MACH_MSGH_BITS(reply_type, dest_type);
        (*msg).msgh_local_port = dest_name;
    } else {
        (*msg).msgh_bits = MACH_MSGH_BITS_OTHER(mbits)
            | MACH_MSGH_BITS(reply_type, MACH_MSG_TYPE_PROTECTED_PAYLOAD);
        (*msg).msgh_local_port = payload; /* union */
    }

    (*msg).msgh_remote_port = reply_name;

    let _ = copyout_dest; /* discriminator already consumed */

    MACH_MSG_SUCCESS
}
