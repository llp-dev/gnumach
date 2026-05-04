//! Port of `ipc/ipc_entry.c`.
//!
//! Primitive functions to manipulate translation entries.

use core::ptr::{addr_of, addr_of_mut, null_mut};

use crate::extern_c::{
    kmem_cache_alloc, kmem_cache_free, rdxtree_insert_alloc_common,
    rdxtree_insert_common, rdxtree_lookup_common, rdxtree_replace_slot,
};
use crate::mach_types::{
    ipc_entry_t, ipc_space_t, kern_return_t, kmem_cache,
    mach_port_name_t, rdxtree_key_t, vm_offset_t, IE_BITS_TYPE_MASK, IE_NULL,
    KERN_INVALID_TASK, KERN_NO_SPACE, KERN_RESOURCE_SHORTAGE, KERN_SUCCESS,
    SIZE_OF_KMEM_CACHE,
};

// ---------------------------------------------------------------------------
//  Globals (defined here, as in C `ipc/ipc_entry.c`)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static mut ipc_entry_cache: kmem_cache = kmem_cache {
    _bytes: [0u8; SIZE_OF_KMEM_CACHE],
};

// ---------------------------------------------------------------------------
//  Macros expanded inline
// ---------------------------------------------------------------------------

#[inline]
unsafe fn ie_alloc() -> ipc_entry_t {
    kmem_cache_alloc(addr_of_mut!(ipc_entry_cache)) as ipc_entry_t
}

#[inline]
unsafe fn ie_free(e: ipc_entry_t) {
    kmem_cache_free(addr_of_mut!(ipc_entry_cache), e as vm_offset_t);
}

/// `IE_BITS_TYPE(bits)`.
#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}

/// `static inline ipc_entry_get` from `ipc/ipc_space.h`.  With the current
/// port.h: `MACH_PORT_MAKE(index, gen) = index`, `IE_BITS_GEN_ONE = 0`,
/// so the generation arithmetic collapses to a no-op.
#[inline]
unsafe fn ipc_entry_get(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
    entryp: *mut ipc_entry_t,
) -> kern_return_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");

    let free_entry = (*space).is_free_list;
    if free_entry == IE_NULL {
        return KERN_NO_SPACE;
    }

    (*space).is_free_list = (*free_entry).index.next_free;
    (*space).is_free_list_size -= 1;

    /*
     *  Initialize the new entry.  We need only
     *  increment the generation number and clear ie_request.
     *  With IE_BITS_GEN_ONE == 0 the generation arithmetic is a no-op.
     */
    let gen: u32 = (*free_entry).ie_bits.wrapping_add(0);
    (*free_entry).ie_bits = gen;
    (*free_entry).index.request = 0;
    let new_name: mach_port_name_t = (*free_entry).ie_name; /* MACH_PORT_MAKE(index, gen) = index */

    crate::kassert!(new_name != 0 && new_name != !0u32, "MACH_PORT_NAME_VALID(new_name)");
    crate::kassert!((*free_entry).ie_object.is_null(), "free_entry->ie_object == IO_NULL");

    (*space).is_size += 1;
    *namep = new_name;
    *entryp = free_entry;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_entry_alloc
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_entry_alloc(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
    entryp: *mut ipc_entry_t,
) -> kern_return_t {
    if (*space).is_active == 0 {
        return KERN_INVALID_TASK;
    }

    let kr = ipc_entry_get(space, namep, entryp);
    if kr == KERN_SUCCESS {
        return kr;
    }

    let entry = ie_alloc();
    if entry == IE_NULL {
        return KERN_RESOURCE_SHORTAGE;
    }

    let mut key: rdxtree_key_t = 0;
    let kr2 = rdxtree_insert_alloc_common(
        addr_of_mut!((*space).is_map),
        entry as *mut core::ffi::c_void,
        &mut key,
        null_mut(),
    );
    if kr2 != 0 {
        ie_free(entry);
        return kr2 as kern_return_t;
    }
    (*space).is_size += 1;

    (*entry).ie_bits = 0;
    (*entry).ie_object = null_mut();
    (*entry).index.request = 0;
    (*entry).ie_name = key as mach_port_name_t;

    *entryp = entry;
    *namep = key as mach_port_name_t;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_entry_alloc_name
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_entry_alloc_name(
    space: ipc_space_t,
    name: mach_port_name_t,
    entryp: *mut ipc_entry_t,
) -> kern_return_t {
    crate::kassert!(name != 0 && name != !0u32, "MACH_PORT_NAME_VALID(name)");

    if (*space).is_active == 0 {
        return KERN_INVALID_TASK;
    }

    let slot = rdxtree_lookup_common(
        addr_of!((*space).is_map),
        name as rdxtree_key_t,
        1,
    ) as *mut *mut core::ffi::c_void;

    let mut entry: ipc_entry_t = IE_NULL;
    if !slot.is_null() {
        entry = *(slot as *mut ipc_entry_t);
    }

    if slot.is_null() || entry == IE_NULL {
        entry = ie_alloc();
        if entry == IE_NULL {
            return KERN_RESOURCE_SHORTAGE;
        }

        (*entry).ie_bits = 0;
        (*entry).ie_object = null_mut();
        (*entry).index.request = 0;
        (*entry).ie_name = name;

        if !slot.is_null() {
            rdxtree_replace_slot(slot, entry as *mut core::ffi::c_void);
        } else {
            let kr = rdxtree_insert_common(
                addr_of_mut!((*space).is_map),
                name as rdxtree_key_t,
                entry as *mut core::ffi::c_void,
                null_mut(),
            );
            if kr != 0 {
                ie_free(entry);
                return kr as kern_return_t;
            }
        }
        (*space).is_size += 1;

        *entryp = entry;
        return KERN_SUCCESS;
    }

    if ie_bits_type((*entry).ie_bits) != 0 {
        /* Used entry.  */
        *entryp = entry;
        return KERN_SUCCESS;
    }

    /* Free entry.  Rip the entry out of the free list.  */
    let mut prevp: *mut ipc_entry_t = addr_of_mut!((*space).is_free_list);
    let mut e: ipc_entry_t = (*space).is_free_list;
    while e != entry {
        prevp = addr_of_mut!((*e).index.next_free);
        e = (*e).index.next_free;
    }

    *prevp = (*entry).index.next_free;
    (*space).is_free_list_size -= 1;

    (*entry).ie_bits = 0;
    crate::kassert!((*entry).ie_object.is_null(), "entry->ie_object == IO_NULL");
    crate::kassert!((*entry).ie_name == name, "entry->ie_name == name");
    (*entry).index.request = 0;

    (*space).is_size += 1;
    *entryp = entry;
    KERN_SUCCESS
}
