//! Port of `ipc/mach_port.c`.
//!
//! Exported kernel calls.  See `mach/mach_port.defs`.

use core::mem::size_of;
use core::ptr::{addr_of_mut, null_mut};

use crate::extern_c::{
    ipc_kobject_set_locked, kmem_free, percpu_array, printf, rdxtree_walk,
    vm_allocate, vm_map_copyin, vm_map_pageable, SoftDebugger,
};
use crate::ipc_init::ipc_kernel_map;
use crate::ipc_object::{
    ipc_object_alloc_dead, ipc_object_alloc_dead_name, ipc_object_copyin,
    ipc_object_copyin_type, ipc_object_copyout_name, ipc_object_rename,
    ipc_object_translate,
};
use crate::ipc_port::{
    ipc_port_alloc, ipc_port_alloc_name, ipc_port_clear_protected_payload,
    ipc_port_nsrequest, ipc_port_pdrequest, ipc_port_set_protected_payload,
    ipc_port_set_qlimit as ipc_port_set_qlimit_inner,
    ipc_port_set_seqno as ipc_port_set_seqno_inner, ipc_port_timestamp,
};
use crate::ipc_pset::{ipc_pset_alloc, ipc_pset_alloc_name, ipc_pset_move};
use crate::ipc_right::{
    ipc_right_dealloc, ipc_right_delta, ipc_right_destroy, ipc_right_dnrequest,
    ipc_right_info, ipc_right_lookup_write,
};
use crate::mach_types::{
    boolean_t, ipc_entry_num_t, ipc_entry_t, ipc_object,
    ipc_port_t, ipc_port_timestamp_t, ipc_pset_t,
    ipc_space_t, host_t, ipc_thread_t, kern_return_t, mach_msg_id_t,
    mach_msg_type_name_t, mach_msg_type_number_t, mach_port_delta_t,
    mach_port_ktype_t, mach_port_mscount_t, mach_port_msgcount_t,
    mach_port_name_t, mach_port_right_t, mach_port_seqno_t,
    mach_port_status_t, mach_port_type_t, mach_port_urefs_t, rdxtree_iter,
    vm_map_copy_t, vm_offset_t, vm_size_t, round_page,
    OFFSETOF_PERCPU_ACTIVE_THREAD, OFFSETOF_TASK_ITK_SPACE,
    OFFSETOF_TASK_NAME, TASK_NAME_SIZE, IO_BITS_KOTYPE, IPS_NULL, IS_NULL,
    IO_DEAD, IKOT_NONE, IKOT_USER_DEVICE, IKO_NULL, HOST_NULL,
    IE_BITS_MAREQUEST, IE_BITS_TYPE_MASK, IE_NULL, KERN_INVALID_ARGUMENT,
    KERN_INVALID_CAPABILITY, KERN_INVALID_HOST, KERN_INVALID_NAME,
    KERN_INVALID_RIGHT, KERN_INVALID_TASK, KERN_INVALID_VALUE,
    KERN_RESOURCE_SHORTAGE, KERN_SUCCESS, MACH_NOTIFY_DEAD_NAME,
    MACH_NOTIFY_NO_SENDERS, MACH_NOTIFY_PORT_DESTROYED, MACH_PORT_NAME_NULL,
    MACH_PORT_QLIMIT_MAX, MACH_PORT_KTYPE_NONE, MACH_PORT_KTYPE_USER_DEVICE,
    MACH_PORT_RIGHT_DEAD_NAME, MACH_PORT_RIGHT_NUMBER, MACH_PORT_RIGHT_PORT_SET,
    MACH_PORT_RIGHT_RECEIVE, MACH_PORT_RIGHT_SEND, MACH_PORT_RIGHT_SEND_ONCE,
    MACH_PORT_TYPE_DNREQUEST, MACH_PORT_TYPE_MAREQUEST, MACH_PORT_TYPE_NONE,
    MACH_PORT_TYPE_PORT_SET, MACH_PORT_TYPE_RECEIVE,
    MACH_PORT_TYPE_SEND_RIGHTS, MACH_MSG_TYPE_PORT_ANY,
    MACH_MSG_TYPE_PORT_ANY_RIGHT, PAGE_SIZE, VM_MAP_COPY_NULL, VM_PROT_NONE,
    VM_PROT_READ, VM_PROT_WRITE,
};

// ---------------------------------------------------------------------------
//  Inline helpers (lock no-ops with NCPUS == 1)
// ---------------------------------------------------------------------------

#[inline]
unsafe fn current_thread() -> ipc_thread_t {
    let base = addr_of_mut!(percpu_array) as *mut u8;
    *(base.add(OFFSETOF_PERCPU_ACTIVE_THREAD) as *mut ipc_thread_t)
}

#[inline]
unsafe fn current_space() -> ipc_space_t {
    let task = (*current_thread()).task as *mut u8;
    *(task.add(OFFSETOF_TASK_ITK_SPACE) as *mut ipc_space_t)
}

#[inline]
unsafe fn current_task_name() -> *const u8 {
    let task = (*current_thread()).task as *mut u8;
    task.add(OFFSETOF_TASK_NAME) as *const u8
}

/// `MACH_PORT_NAME_VALID(name) = name != 0 && name != !0`.
#[inline]
fn mach_port_name_valid(name: mach_port_name_t) -> bool {
    name != 0 && name != !0u32
}

use crate::locks::{
    imq_lock, imq_unlock, ip_active, ip_lock, ip_unlock, ips_active, ips_lock,
    ips_unlock, is_read_lock, is_read_unlock, is_write_unlock,
};

#[inline]
unsafe fn ip_kotype(port: ipc_port_t) -> u32 {
    (*port).ip_target.ipt_object.io_bits & IO_BITS_KOTYPE
}

#[inline]
unsafe fn ips_check_unlock(pset: ipc_pset_t) {
    crate::locks::io_check_unlock(addr_of_mut!((*pset).ips_target.ipt_object));
}

#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}

/// `IP_TIMESTAMP_ORDER(one, two)` — true if `one` happened before `two`.
#[inline]
fn ip_timestamp_order(one: ipc_port_timestamp_t, two: ipc_port_timestamp_t) -> bool {
    ((one as i32).wrapping_sub(two as i32)) < 0
}

#[inline]
unsafe fn ipc_port_translate_receive(
    space: ipc_space_t,
    name: mach_port_name_t,
    portp: *mut ipc_port_t,
) -> kern_return_t {
    ipc_object_translate(
        space,
        name,
        MACH_PORT_RIGHT_RECEIVE,
        portp as *mut *mut ipc_object,
    )
}

#[inline]
unsafe fn ipc_entry_lookup(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> ipc_entry_t {
    let entry = crate::extern_c::rdxtree_lookup_common(
        core::ptr::addr_of!((*space).is_map),
        name as crate::mach_types::rdxtree_key_t,
        0,
    ) as ipc_entry_t;
    if entry == IE_NULL || ((*entry).ie_bits & IE_BITS_TYPE_MASK) == 0 {
        IE_NULL
    } else {
        entry
    }
}

/// Local helper: `MACH_PORT_TYPE(right) = 1 << (right + 16)`.
#[inline]
const fn port_type_for(right: u32) -> u32 {
    1u32 << (right + 16)
}

/// `mach_port_names_helper` — see C original.
unsafe fn mach_port_names_helper(
    timestamp: ipc_port_timestamp_t,
    entry: ipc_entry_t,
    name: mach_port_name_t,
    names: *mut mach_port_name_t,
    types: *mut mach_port_type_t,
    actualp: *mut ipc_entry_num_t,
) {
    let mut bits = (*entry).ie_bits;
    let mut request = (*entry).index.request;

    if (bits & MACH_PORT_TYPE_SEND_RIGHTS) != 0 {
        let port = (*entry).ie_object as ipc_port_t;
        ip_lock(port);
        let died = !ip_active(port)
            && ip_timestamp_order((*port).data.timestamp, timestamp);
        ip_unlock(port);

        if died {
            /* pretend this is a dead-name entry */
            bits &= !(IE_BITS_TYPE_MASK | IE_BITS_MAREQUEST);
            bits |= crate::mach_types::MACH_PORT_TYPE_DEAD_NAME;
            if request != 0 {
                bits += 1;
            }
            request = 0;
        }
    }

    let mut typ = ie_bits_type(bits);
    if request != 0 {
        typ |= MACH_PORT_TYPE_DNREQUEST;
    }
    if (bits & IE_BITS_MAREQUEST) != 0 {
        typ |= MACH_PORT_TYPE_MAREQUEST;
    }

    let actual = *actualp;
    *names.add(actual as usize) = name;
    *types.add(actual as usize) = typ;
    *actualp = actual + 1;
}

// ---------------------------------------------------------------------------
//  mach_port_names
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_names(
    space: ipc_space_t,
    namesp: *mut *mut mach_port_name_t,
    namesCnt: *mut mach_msg_type_number_t,
    typesp: *mut *mut mach_port_type_t,
    typesCnt: *mut mach_msg_type_number_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut size: vm_size_t = 0;
    let mut addr1: vm_offset_t = 0;
    let mut addr2: vm_offset_t = 0;
    let mut bound: ipc_entry_num_t;

    loop {
        is_read_lock(space);
        if (*space).is_active == 0 {
            is_read_unlock(space);
            if size != 0 {
                kmem_free(ipc_kernel_map, addr1, size);
                kmem_free(ipc_kernel_map, addr2, size);
            }
            return KERN_INVALID_TASK;
        }

        bound = (*space).is_size as ipc_entry_num_t;
        let size_needed: vm_size_t =
            round_page(bound as vm_size_t * size_of::<mach_port_name_t>() as vm_size_t);

        if size_needed <= size {
            break;
        }

        is_read_unlock(space);

        if size != 0 {
            kmem_free(ipc_kernel_map, addr1, size);
            kmem_free(ipc_kernel_map, addr2, size);
        }
        size = size_needed;

        let mut kr = vm_allocate(ipc_kernel_map, &mut addr1, size, 1);
        if kr != KERN_SUCCESS {
            return KERN_RESOURCE_SHORTAGE;
        }

        kr = vm_allocate(ipc_kernel_map, &mut addr2, size, 1);
        if kr != KERN_SUCCESS {
            kmem_free(ipc_kernel_map, addr1, size);
            return KERN_RESOURCE_SHORTAGE;
        }

        let _ = vm_map_pageable(
            ipc_kernel_map,
            addr1,
            addr1 + size,
            VM_PROT_READ | VM_PROT_WRITE,
            1,
            1,
        );
        let _ = vm_map_pageable(
            ipc_kernel_map,
            addr2,
            addr2 + size,
            VM_PROT_READ | VM_PROT_WRITE,
            1,
            1,
        );
    }
    /* space is read-locked and active */

    let names = addr1 as *mut mach_port_name_t;
    let types = addr2 as *mut mach_port_type_t;
    let mut actual: ipc_entry_num_t = 0;

    let timestamp = ipc_port_timestamp();

    let mut iter = rdxtree_iter { node: null_mut(), key: !0 };
    let mut entry = rdxtree_walk(addr_of_mut!((*space).is_map), &mut iter)
        as ipc_entry_t;
    while !entry.is_null() {
        let bits = (*entry).ie_bits;
        if ie_bits_type(bits) != MACH_PORT_TYPE_NONE {
            mach_port_names_helper(
                timestamp,
                entry,
                (*entry).ie_name,
                names,
                types,
                &mut actual,
            );
        }
        entry = rdxtree_walk(addr_of_mut!((*space).is_map), &mut iter)
            as ipc_entry_t;
    }
    crate::kassert!(actual < bound, "actual < bound");
    let _ = bound;
    is_read_unlock(space);

    let memory1: vm_map_copy_t;
    let memory2: vm_map_copy_t;

    if actual == 0 {
        memory1 = VM_MAP_COPY_NULL;
        memory2 = VM_MAP_COPY_NULL;

        if size != 0 {
            kmem_free(ipc_kernel_map, addr1, size);
            kmem_free(ipc_kernel_map, addr2, size);
        }
    } else {
        let size_used = round_page(
            actual as vm_size_t * size_of::<mach_port_name_t>() as vm_size_t,
        );

        let _ = vm_map_pageable(ipc_kernel_map, addr1, addr1 + size_used, VM_PROT_NONE, 1, 1);
        let _ = vm_map_pageable(ipc_kernel_map, addr2, addr2 + size_used, VM_PROT_NONE, 1, 1);

        let mut copy1: vm_map_copy_t = null_mut();
        let mut copy2: vm_map_copy_t = null_mut();
        let _ = vm_map_copyin(ipc_kernel_map, addr1, size_used, 1, &mut copy1);
        let _ = vm_map_copyin(ipc_kernel_map, addr2, size_used, 1, &mut copy2);
        memory1 = copy1;
        memory2 = copy2;

        if size_used != size {
            kmem_free(ipc_kernel_map, addr1 + size_used, size - size_used);
            kmem_free(ipc_kernel_map, addr2 + size_used, size - size_used);
        }
    }

    *namesp = memory1 as *mut mach_port_name_t;
    *namesCnt = actual;
    *typesp = memory2 as *mut mach_port_type_t;
    *typesCnt = actual;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_type
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_type(
    space: ipc_space_t,
    name: mach_port_name_t,
    typep: *mut mach_port_type_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut entry: ipc_entry_t = IE_NULL;
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* space is write-locked and active */

    let mut urefs: mach_port_urefs_t = 0;
    let kr = ipc_right_info(space, name, entry, typep, &mut urefs);
    if kr == KERN_SUCCESS {
        is_write_unlock(space);
    }
    /* space is unlocked */
    kr
}

// ---------------------------------------------------------------------------
//  mach_port_rename
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_rename(
    space: ipc_space_t,
    oname: mach_port_name_t,
    nname: mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if nname == 0 || nname == !0u32 {
        return KERN_INVALID_VALUE;
    }
    ipc_object_rename(space, oname, nname)
}

// ---------------------------------------------------------------------------
//  mach_port_allocate_name / mach_port_allocate
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_allocate_name(
    space: ipc_space_t,
    right: mach_port_right_t,
    name: mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if name == 0 || name == !0u32 {
        return KERN_INVALID_VALUE;
    }

    if right == MACH_PORT_RIGHT_RECEIVE {
        let mut port: ipc_port_t = null_mut();
        let kr = ipc_port_alloc_name(space, name, &mut port);
        if kr == KERN_SUCCESS {
            ip_unlock(port);
        }
        kr
    } else if right == MACH_PORT_RIGHT_PORT_SET {
        let mut pset: ipc_pset_t = null_mut();
        let kr = ipc_pset_alloc_name(space, name, &mut pset);
        if kr == KERN_SUCCESS {
            ips_unlock(pset);
        }
        kr
    } else if right == MACH_PORT_RIGHT_DEAD_NAME {
        ipc_object_alloc_dead_name(space, name)
    } else {
        KERN_INVALID_VALUE
    }
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_allocate(
    space: ipc_space_t,
    right: mach_port_right_t,
    namep: *mut mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if right == MACH_PORT_RIGHT_RECEIVE {
        let mut port: ipc_port_t = null_mut();
        let kr = ipc_port_alloc(space, namep, &mut port);
        if kr == KERN_SUCCESS {
            ip_unlock(port);
        }
        kr
    } else if right == MACH_PORT_RIGHT_PORT_SET {
        let mut pset: ipc_pset_t = null_mut();
        let kr = ipc_pset_alloc(space, namep, &mut pset);
        if kr == KERN_SUCCESS {
            ips_unlock(pset);
        }
        kr
    } else if right == MACH_PORT_RIGHT_DEAD_NAME {
        ipc_object_alloc_dead(space, namep)
    } else {
        KERN_INVALID_VALUE
    }
}

// ---------------------------------------------------------------------------
//  mach_port_destroy / mach_port_deallocate
// ---------------------------------------------------------------------------

#[no_mangle]
pub static mach_port_deallocate_debug: boolean_t = 0;

#[no_mangle]
pub unsafe extern "C" fn mach_port_destroy(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut entry: ipc_entry_t = IE_NULL;
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        if mach_port_name_valid(name) && space == current_space() {
            printf(
                b"task %.*s destroying a bogus port %lu, most probably a bug.\n\0".as_ptr() as *const _,
                TASK_NAME_SIZE as core::ffi::c_int,
                current_task_name(),
                name as core::ffi::c_ulong,
            );
            if mach_port_deallocate_debug != 0 {
                SoftDebugger(b"mach_port_deallocate\0".as_ptr() as *const _);
            }
        }
        return kr;
    }
    /* space is write-locked and active */
    ipc_right_destroy(space, name, entry)
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_deallocate(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut entry: ipc_entry_t = IE_NULL;
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        if mach_port_name_valid(name) && space == current_space() {
            printf(
                b"task %.*s deallocating a bogus port %lu, most probably a bug.\n\0".as_ptr() as *const _,
                TASK_NAME_SIZE as core::ffi::c_int,
                current_task_name(),
                name as core::ffi::c_ulong,
            );
            if mach_port_deallocate_debug != 0 {
                SoftDebugger(b"mach_port_deallocate\0".as_ptr() as *const _);
            }
        }
        return kr;
    }
    /* space is write-locked */
    ipc_right_dealloc(space, name, entry)
}

// ---------------------------------------------------------------------------
//  mach_port_get_refs
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_get_refs(
    space: ipc_space_t,
    name: mach_port_name_t,
    right: mach_port_right_t,
    urefsp: *mut mach_port_urefs_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if right >= MACH_PORT_RIGHT_NUMBER {
        return KERN_INVALID_VALUE;
    }

    let mut entry: ipc_entry_t = IE_NULL;
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* space is write-locked and active */

    let mut typ: mach_port_type_t = 0;
    let mut urefs: mach_port_urefs_t = 0;
    let kr = ipc_right_info(space, name, entry, &mut typ, &mut urefs);
    if kr != KERN_SUCCESS {
        return kr; /* space is unlocked */
    }
    is_write_unlock(space);

    if (typ & port_type_for(right)) != 0 {
        match right {
            x if x == MACH_PORT_RIGHT_SEND_ONCE
                || x == MACH_PORT_RIGHT_PORT_SET
                || x == MACH_PORT_RIGHT_RECEIVE => *urefsp = 1,
            x if x == MACH_PORT_RIGHT_DEAD_NAME
                || x == MACH_PORT_RIGHT_SEND => *urefsp = urefs,
            _ => *urefsp = 0,
        }
    } else {
        *urefsp = 0;
    }

    kr
}

// ---------------------------------------------------------------------------
//  mach_port_mod_refs
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_mod_refs(
    space: ipc_space_t,
    name: mach_port_name_t,
    right: mach_port_right_t,
    delta: mach_port_delta_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if right >= MACH_PORT_RIGHT_NUMBER {
        return KERN_INVALID_VALUE;
    }

    let mut entry: ipc_entry_t = IE_NULL;
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        if mach_port_name_valid(name) && space == current_space() {
            let prefix = if delta < 0 {
                b"task %.*s decreasing a bogus port %u by %d, most probably a bug.\n\0".as_ptr()
            } else {
                b"task %.*s increasing a bogus port %u by %d, most probably a bug.\n\0".as_ptr()
            };
            printf(
                prefix as *const _,
                TASK_NAME_SIZE as core::ffi::c_int,
                current_task_name(),
                name as core::ffi::c_uint,
                if delta < 0 { -delta } else { delta },
            );
            if mach_port_deallocate_debug != 0 {
                SoftDebugger(b"mach_port_mod_refs\0".as_ptr() as *const _);
            }
        }
        return kr;
    }
    /* space is write-locked and active */

    ipc_right_delta(space, name, entry, right, delta)
}

// ---------------------------------------------------------------------------
//  mach_port_set_qlimit / mscount / seqno
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_set_qlimit(
    space: ipc_space_t,
    name: mach_port_name_t,
    qlimit: mach_port_msgcount_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if qlimit > MACH_PORT_QLIMIT_MAX {
        return KERN_INVALID_VALUE;
    }

    let mut port: ipc_port_t = null_mut();
    let kr = ipc_port_translate_receive(space, name, &mut port);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* port is locked and active */

    ipc_port_set_qlimit_inner(port, qlimit);

    ip_unlock(port);
    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_set_mscount(
    space: ipc_space_t,
    name: mach_port_name_t,
    mscount: mach_port_mscount_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    let mut port: ipc_port_t = null_mut();
    let kr = ipc_port_translate_receive(space, name, &mut port);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* port is locked and active.  ipc_port_set_mscount macro: just sets. */
    (*port).ip_mscount = mscount;

    ip_unlock(port);
    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_set_seqno(
    space: ipc_space_t,
    name: mach_port_name_t,
    seqno: mach_port_seqno_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    let mut port: ipc_port_t = null_mut();
    let kr = ipc_port_translate_receive(space, name, &mut port);
    if kr != KERN_SUCCESS {
        return kr;
    }
    ipc_port_set_seqno_inner(port, seqno);
    ip_unlock(port);
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_get_set_status
// ---------------------------------------------------------------------------

unsafe fn mach_port_gst_helper(
    pset: ipc_pset_t,
    port: ipc_port_t,
    maxnames: ipc_entry_num_t,
    names: *mut mach_port_name_t,
    actualp: *mut ipc_entry_num_t,
) {
    ip_lock(port);
    let name = (*port).ip_target.ipt_name;
    let ip_pset = (*port).ip_pset;
    ip_unlock(port);

    if pset == ip_pset {
        let actual = *actualp;
        if actual < maxnames {
            *names.add(actual as usize) = name;
        }
        *actualp = actual + 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_get_set_status(
    space: ipc_space_t,
    name: mach_port_name_t,
    members: *mut *mut mach_port_name_t,
    membersCnt: *mut mach_msg_type_number_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut size: vm_size_t = PAGE_SIZE;
    let mut actual: ipc_entry_num_t;
    let mut addr: vm_offset_t = 0;
    let memory: vm_map_copy_t;

    loop {
        let mut kr = vm_allocate(ipc_kernel_map, &mut addr, size, 1);
        if kr != KERN_SUCCESS {
            return KERN_RESOURCE_SHORTAGE;
        }
        let _ = vm_map_pageable(
            ipc_kernel_map,
            addr,
            addr + size,
            VM_PROT_READ | VM_PROT_WRITE,
            1,
            1,
        );

        let mut entry: ipc_entry_t = IE_NULL;
        kr = ipc_right_lookup_write(space, name, &mut entry);
        if kr != KERN_SUCCESS {
            kmem_free(ipc_kernel_map, addr, size);
            return kr;
        }
        /* space is read-locked and active */

        if ie_bits_type((*entry).ie_bits) != MACH_PORT_TYPE_PORT_SET {
            is_read_unlock(space);
            kmem_free(ipc_kernel_map, addr, size);
            return KERN_INVALID_RIGHT;
        }

        let pset = (*entry).ie_object as ipc_pset_t;
        let names = addr as *mut mach_port_name_t;
        let maxnames = (size / size_of::<mach_port_name_t>() as vm_size_t)
            as ipc_entry_num_t;
        actual = 0;

        let mut iter = rdxtree_iter { node: null_mut(), key: !0 };
        let mut ientry = rdxtree_walk(addr_of_mut!((*space).is_map), &mut iter)
            as ipc_entry_t;
        while !ientry.is_null() {
            let bits = (*ientry).ie_bits;
            if (bits & MACH_PORT_TYPE_RECEIVE) != 0 {
                let port = (*ientry).ie_object as ipc_port_t;
                mach_port_gst_helper(pset, port, maxnames, names, &mut actual);
            }
            ientry = rdxtree_walk(addr_of_mut!((*space).is_map), &mut iter)
                as ipc_entry_t;
        }

        is_read_unlock(space);

        if actual <= maxnames {
            break;
        }

        kmem_free(ipc_kernel_map, addr, size);
        size = round_page(
            actual as vm_size_t * size_of::<mach_port_name_t>() as vm_size_t,
        ) + PAGE_SIZE;
    }

    if actual == 0 {
        memory = VM_MAP_COPY_NULL;
        kmem_free(ipc_kernel_map, addr, size);
    } else {
        let size_used = round_page(
            actual as vm_size_t * size_of::<mach_port_name_t>() as vm_size_t,
        );
        let _ = vm_map_pageable(
            ipc_kernel_map,
            addr,
            addr + size_used,
            VM_PROT_NONE,
            1,
            1,
        );
        let mut copy: vm_map_copy_t = null_mut();
        let _ = vm_map_copyin(ipc_kernel_map, addr, size_used, 1, &mut copy);
        memory = copy;
        if size_used != size {
            kmem_free(ipc_kernel_map, addr + size_used, size - size_used);
        }
    }

    *members = memory as *mut mach_port_name_t;
    *membersCnt = actual;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_move_member
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_move_member(
    space: ipc_space_t,
    member: mach_port_name_t,
    after: mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut entry: ipc_entry_t = IE_NULL;
    let kr = ipc_right_lookup_write(space, member, &mut entry);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* space is read-locked and active */

    if ((*entry).ie_bits & MACH_PORT_TYPE_RECEIVE) == 0 {
        is_read_unlock(space);
        return KERN_INVALID_RIGHT;
    }

    let port = (*entry).ie_object as ipc_port_t;
    let nset: ipc_pset_t;

    if after == MACH_PORT_NAME_NULL {
        nset = IPS_NULL;
    } else {
        let entry2 = ipc_entry_lookup(space, after);
        if entry2 == IE_NULL {
            is_read_unlock(space);
            return KERN_INVALID_NAME;
        }
        if ((*entry2).ie_bits & MACH_PORT_TYPE_PORT_SET) == 0 {
            is_read_unlock(space);
            return KERN_INVALID_RIGHT;
        }
        nset = (*entry2).ie_object as ipc_pset_t;
    }

    ipc_pset_move(space, port, nset)
}

// ---------------------------------------------------------------------------
//  mach_port_request_notification
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_request_notification(
    space: ipc_space_t,
    name: mach_port_name_t,
    id: mach_msg_id_t,
    sync: mach_port_mscount_t,
    notify: ipc_port_t,
    previousp: *mut ipc_port_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if notify == (!0usize) as ipc_port_t {
        /* IP_DEAD */
        return KERN_INVALID_CAPABILITY;
    }

    if id == MACH_NOTIFY_PORT_DESTROYED {
        if sync != 0 {
            return KERN_INVALID_VALUE;
        }
        let mut port: ipc_port_t = null_mut();
        let kr = ipc_port_translate_receive(space, name, &mut port);
        if kr != KERN_SUCCESS {
            return kr;
        }
        let mut previous: ipc_port_t = null_mut();
        ipc_port_pdrequest(port, notify, &mut previous);
        *previousp = previous;
        KERN_SUCCESS
    } else if id == MACH_NOTIFY_NO_SENDERS {
        let mut port: ipc_port_t = null_mut();
        let kr = ipc_port_translate_receive(space, name, &mut port);
        if kr != KERN_SUCCESS {
            return kr;
        }
        ipc_port_nsrequest(port, sync, notify, previousp);
        KERN_SUCCESS
    } else if id == MACH_NOTIFY_DEAD_NAME {
        ipc_right_dnrequest(
            space,
            name,
            (sync != 0) as boolean_t,
            notify,
            previousp,
        )
    } else {
        KERN_INVALID_VALUE
    }
}

// ---------------------------------------------------------------------------
//  mach_port_insert_right / mach_port_extract_right
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_insert_right(
    space: ipc_space_t,
    name: mach_port_name_t,
    poly: ipc_port_t,
    polyPoly: mach_msg_type_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if name == 0 || name == !0u32 || !MACH_MSG_TYPE_PORT_ANY_RIGHT(polyPoly) {
        return KERN_INVALID_VALUE;
    }
    if (poly as usize) == 0 || (poly as usize) == !0usize {
        return KERN_INVALID_CAPABILITY;
    }
    ipc_object_copyout_name(
        space,
        poly as *mut ipc_object,
        polyPoly,
        0,
        name,
    )
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_extract_right(
    space: ipc_space_t,
    name: mach_port_name_t,
    msgt_name: mach_msg_type_name_t,
    poly: *mut ipc_port_t,
    polyPoly: *mut mach_msg_type_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if !MACH_MSG_TYPE_PORT_ANY(msgt_name) {
        return KERN_INVALID_VALUE;
    }
    let kr = ipc_object_copyin(
        space,
        name,
        msgt_name,
        poly as *mut *mut ipc_object,
    );
    if kr == KERN_SUCCESS {
        *polyPoly = ipc_object_copyin_type(msgt_name);
    }
    kr
}

// ---------------------------------------------------------------------------
//  mach_port_get_receive_status
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_get_receive_status(
    space: ipc_space_t,
    name: mach_port_name_t,
    statusp: *mut mach_port_status_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut port: ipc_port_t = null_mut();
    let kr = ipc_port_translate_receive(space, name, &mut port);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* port is locked and active */

    let mut went_to_no_pset = false;
    if (*port).ip_pset != IPS_NULL {
        let pset = (*port).ip_pset;
        ips_lock(pset);
        if !ips_active(pset) {
            crate::ipc_pset::ipc_pset_remove(pset, port);
            ips_check_unlock(pset);
            went_to_no_pset = true;
        } else {
            (*statusp).mps_pset = (*pset).ips_target.ipt_name;
            imq_lock(addr_of_mut!((*pset).ips_target.ipt_messages));
            (*statusp).mps_seqno = (*port).ip_seqno;
            imq_unlock(addr_of_mut!((*pset).ips_target.ipt_messages));
            ips_unlock(pset);
        }
    } else {
        went_to_no_pset = true;
    }

    if went_to_no_pset {
        (*statusp).mps_pset = MACH_PORT_NAME_NULL;
        imq_lock(addr_of_mut!((*port).ip_target.ipt_messages));
        (*statusp).mps_seqno = (*port).ip_seqno;
        imq_unlock(addr_of_mut!((*port).ip_target.ipt_messages));
    }

    (*statusp).mps_mscount = (*port).ip_mscount;
    (*statusp).mps_qlimit = (*port).ip_qlimit;
    (*statusp).mps_msgcount = (*port).ip_msgcount;
    (*statusp).mps_sorights = (*port).ip_sorights;
    (*statusp).mps_srights = ((*port).ip_srights > 0) as boolean_t;
    (*statusp).mps_pdrequest = (!(*port).ip_pdrequest.is_null()) as boolean_t;
    (*statusp).mps_nsrequest = (!(*port).ip_nsrequest.is_null()) as boolean_t;
    ip_unlock(port);

    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_set_protected_payload / mach_port_clear_protected_payload
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_set_protected_payload(
    space: ipc_space_t,
    name: mach_port_name_t,
    payload: u32,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    let mut port: ipc_port_t = null_mut();
    let kr = ipc_port_translate_receive(space, name, &mut port);
    if kr != KERN_SUCCESS {
        return kr;
    }
    ipc_port_set_protected_payload(port, payload);
    ip_unlock(port);
    KERN_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn mach_port_clear_protected_payload(
    space: ipc_space_t,
    name: mach_port_name_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    let mut port: ipc_port_t = null_mut();
    let kr = ipc_port_translate_receive(space, name, &mut port);
    if kr != KERN_SUCCESS {
        return kr;
    }
    ipc_port_clear_protected_payload(port);
    ip_unlock(port);
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_set_ktype
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_set_ktype(
    host_priv: host_t,
    space: ipc_space_t,
    name: mach_port_name_t,
    right: mach_port_right_t,
    ktype: mach_port_ktype_t,
) -> kern_return_t {
    if host_priv == HOST_NULL {
        return KERN_INVALID_HOST;
    }
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }
    if ktype != MACH_PORT_KTYPE_NONE && ktype != MACH_PORT_KTYPE_USER_DEVICE {
        return KERN_INVALID_ARGUMENT;
    }

    let mut port: ipc_port_t = null_mut();
    let kr = ipc_object_translate(
        space,
        name,
        right,
        &mut port as *mut ipc_port_t as *mut *mut ipc_object,
    );
    if kr != KERN_SUCCESS {
        return kr;
    }

    let kotype = ip_kotype(port);
    let kr = if kotype == IKOT_NONE || kotype == IKOT_USER_DEVICE {
        ipc_kobject_set_locked(
            port,
            IKO_NULL,
            if ktype == MACH_PORT_KTYPE_NONE {
                IKOT_NONE
            } else {
                IKOT_USER_DEVICE
            },
        );
        KERN_SUCCESS
    } else {
        KERN_INVALID_ARGUMENT
    };

    ip_unlock(port);
    let _ = (IO_DEAD, KERN_INVALID_NAME, MACH_PORT_NAME_NULL);
    kr
}
