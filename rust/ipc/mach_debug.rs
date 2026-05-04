//! Port of `ipc/mach_debug.c`.
//!
//! Exported kernel calls for IPC introspection.  See
//! `mach_debug/mach_debug.defs`.

use core::mem::size_of;
use core::ptr::null_mut;

use crate::extern_c::{kmem_alloc_pageable, kmem_free, vm_map_copyin};
use crate::ipc_init::ipc_kernel_map;
use crate::ipc_marequest::{hash_info_bucket_t, ipc_marequest_info};
use crate::ipc_object::ipc_object_translate;
use crate::ipc_right::ipc_right_lookup_write;
use crate::mach_types::{
    host_t, ipc_object, ipc_port_request_t, ipc_port_t, ipc_space_t,
    kern_return_t, mach_port_name_t, mach_port_rights_t, vm_map_copy_t,
    vm_offset_t, vm_size_t, round_page, HOST_NULL, IS_NULL,
    KERN_INVALID_HOST, KERN_INVALID_RIGHT, KERN_INVALID_TASK,
    KERN_RESOURCE_SHORTAGE, KERN_SUCCESS, MACH_PORT_NAME_NULL,
    MACH_PORT_RIGHT_RECEIVE, MACH_PORT_TYPE_SEND_RECEIVE, IO_BITS_KOTYPE,
};

use crate::locks::{ip_active, ip_lock, ip_unlock, is_read_unlock};

#[inline]
unsafe fn ip_kotype(port: ipc_port_t) -> u32 {
    (*port).ip_target.ipt_object.io_bits & IO_BITS_KOTYPE
}

/// `ipc_port_translate_receive(space, name, &port)` macro:
/// `ipc_object_translate(space, name, MACH_PORT_RIGHT_RECEIVE, &port)`.
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

// ---------------------------------------------------------------------------
//  mach_port_get_srights
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_get_srights(
    space: ipc_space_t,
    name: mach_port_name_t,
    srightsp: *mut mach_port_rights_t,
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

    let srights = (*port).ip_srights;
    ip_unlock(port);

    *srightsp = srights;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  host_ipc_marequest_info
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn host_ipc_marequest_info(
    host: host_t,
    maxp: *mut u32,
    infop: *mut *mut hash_info_bucket_t,
    countp: *mut u32,
) -> kern_return_t {
    if host == HOST_NULL {
        return KERN_INVALID_HOST;
    }

    let mut addr: vm_offset_t = 0;
    let mut size: vm_size_t = 0;
    let mut info: *mut hash_info_bucket_t = *infop;
    let mut potential: u32 = *countp;
    let actual: u32;

    loop {
        let cur = ipc_marequest_info(maxp, info, potential);
        if cur <= potential {
            actual = cur;
            break;
        }

        /* allocate more memory */
        if info != *infop {
            kmem_free(ipc_kernel_map, addr, size);
        }

        size = round_page(cur as vm_size_t * size_of::<hash_info_bucket_t>() as vm_size_t);
        let kr = kmem_alloc_pageable(ipc_kernel_map, &mut addr, size);
        if kr != KERN_SUCCESS {
            return KERN_RESOURCE_SHORTAGE;
        }

        info = addr as *mut hash_info_bucket_t;
        potential = (size / size_of::<hash_info_bucket_t>() as vm_size_t) as u32;
    }

    if info == *infop {
        /* data fit in-line; nothing to deallocate */
        *countp = actual;
    } else if actual == 0 {
        kmem_free(ipc_kernel_map, addr, size);
        *countp = 0;
    } else {
        let used = round_page(
            actual as vm_size_t * size_of::<hash_info_bucket_t>() as vm_size_t,
        );

        if used != size {
            kmem_free(ipc_kernel_map, addr + used, size - used);
        }

        let mut copy: vm_map_copy_t = null_mut();
        let kr = vm_map_copyin(ipc_kernel_map, addr, used, 1 /* TRUE */, &mut copy);
        crate::kassert!(kr == KERN_SUCCESS, "kr == KERN_SUCCESS");

        *infop = copy as *mut hash_info_bucket_t;
        *countp = actual;
    }

    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_dnrequest_info
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_dnrequest_info(
    space: ipc_space_t,
    name: mach_port_name_t,
    totalp: *mut u32,
    usedp: *mut u32,
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

    let total: u32;
    let mut used: u32;

    let dnrequests: ipc_port_request_t = (*port).ip_dnrequests;
    if dnrequests.is_null() {
        total = 0;
        used = 0;
    } else {
        total = (*(*dnrequests).name.size).its_size;
        used = 0;
        let mut index: u32 = 1;
        while index < total {
            let ipr = dnrequests.add(index as usize);
            if (*ipr).name.name != MACH_PORT_NAME_NULL {
                used += 1;
            }
            index += 1;
        }
    }
    ip_unlock(port);

    *totalp = total;
    *usedp = used;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_port_kernel_object
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_port_kernel_object(
    space: ipc_space_t,
    name: mach_port_name_t,
    typep: *mut u32,
    addrp: *mut vm_offset_t,
) -> kern_return_t {
    if space == IS_NULL {
        return KERN_INVALID_TASK;
    }

    let mut entry: crate::mach_types::ipc_entry_t = null_mut();
    /* `ipc_right_lookup_read` is `#define`d to `ipc_right_lookup_write`. */
    let kr = ipc_right_lookup_write(space, name, &mut entry);
    if kr != KERN_SUCCESS {
        return kr;
    }
    /* space is read-locked and active */

    if ((*entry).ie_bits & MACH_PORT_TYPE_SEND_RECEIVE) == 0 {
        is_read_unlock(space);
        return KERN_INVALID_RIGHT;
    }

    let port: ipc_port_t = (*entry).ie_object as ipc_port_t;
    crate::kassert!(!port.is_null(), "port != IP_NULL");

    ip_lock(port);
    is_read_unlock(space);

    if !ip_active(port) {
        ip_unlock(port);
        return KERN_INVALID_RIGHT;
    }

    *typep = ip_kotype(port);
    *addrp = (*port).ip_kobject;
    ip_unlock(port);
    KERN_SUCCESS
}
