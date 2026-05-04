//! Port of `ipc/ipc_space.c`.
//!
//! Functions to manipulate IPC capability spaces.

use core::ptr::{addr_of_mut, null_mut};

use crate::extern_c::{
    kmem_cache_alloc, kmem_cache_free, lock_init,
    rdxtree_insert_common, rdxtree_remove_all, rdxtree_walk,
};
use crate::ipc_right::ipc_right_clean;
use crate::mach_types::{
    ipc_entry, ipc_entry_index_u, ipc_entry_t, ipc_space, ipc_space_t,
    kern_return_t, kmem_cache, mach_port_name_t, rdxtree, rdxtree_iter,
    rdxtree_key_t, vm_offset_t, IE_BITS_TYPE_MASK, IS_NULL,
    KERN_RESOURCE_SHORTAGE, KERN_SUCCESS, MACH_PORT_NAME_NULL,
    SIZE_OF_KMEM_CACHE,
};

// ---------------------------------------------------------------------------
//  Globals (defined here, as in C `ipc/ipc_space.c`)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static mut ipc_space_cache: kmem_cache = kmem_cache {
    _bytes: [0u8; SIZE_OF_KMEM_CACHE],
};

#[no_mangle]
pub static mut ipc_space_kernel: ipc_space_t = core::ptr::null_mut();

#[no_mangle]
pub static mut ipc_space_reply: ipc_space_t = core::ptr::null_mut();

/// `struct ipc_entry zero_entry;` — place-holder for the zeroth entry.
#[no_mangle]
pub static mut zero_entry: ipc_entry = ipc_entry {
    ie_name: 0,
    ie_bits: 0,
    ie_object: core::ptr::null_mut(),
    index: ipc_entry_index_u { request: 0 },
};

// ---------------------------------------------------------------------------
//  Macros expanded inline
// ---------------------------------------------------------------------------

#[inline]
unsafe fn is_alloc() -> ipc_space_t {
    kmem_cache_alloc(addr_of_mut!(ipc_space_cache)) as ipc_space_t
}

#[inline]
unsafe fn is_free(s: ipc_space_t) {
    kmem_cache_free(addr_of_mut!(ipc_space_cache), s as vm_offset_t);
}

/// `is_ref_lock_init(is)` → `simple_lock_init(...)` → no-op with NCPUS == 1.
#[inline]
unsafe fn is_ref_lock_init(_s: ipc_space_t) {}

/// `is_lock_init(is)` → `lock_init(&is->is_lock_data, TRUE)`.
#[inline]
unsafe fn is_lock_init(space: ipc_space_t) {
    lock_init(addr_of_mut!((*space).is_lock_data), 1 /* TRUE */);
}

#[inline]
unsafe fn is_write_lock(space: ipc_space_t) {
    crate::extern_c::lock_write(addr_of_mut!((*space).is_lock_data));
}

#[inline]
unsafe fn is_write_unlock(space: ipc_space_t) {
    crate::extern_c::lock_done(addr_of_mut!((*space).is_lock_data));
}

/// `rdxtree_init(tree)` → height = 0, root = NULL.  Both fields combined are
/// 8 bytes (matches our opaque mirror), so zero the storage.
#[inline]
unsafe fn rdxtree_init(tree: *mut rdxtree) {
    (*tree)._bytes = [0u8; 8];
}

/// `rdxtree_iter_init(iter)`: node = NULL, key = (rdxtree_key_t)-1.
#[inline]
unsafe fn rdxtree_iter_init(iter: *mut rdxtree_iter) {
    (*iter).node = null_mut();
    (*iter).key = !0;
}

#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}

/// `MACH_PORT_MAKEB(index, bits) = MACH_PORT_MAKE(index, IE_BITS_GEN(bits)) =
/// index` (current `port.h` collapses generations to 0).
#[inline]
fn mach_port_makeb(index: mach_port_name_t, _bits: u32) -> mach_port_name_t {
    index
}

#[inline]
unsafe fn ie_free(e: ipc_entry_t) {
    kmem_cache_free(
        addr_of_mut!(crate::ipc_entry::ipc_entry_cache),
        e as vm_offset_t,
    );
}

/// `is_release(space)` — Rust mirror of the inline release macro.
#[inline]
unsafe fn is_release_macro(space: ipc_space_t) {
    /* simple_lock(&space->is_ref_lock_data) — no-op with NCPUS==1 */
    crate::kassert!((*space).is_references > 0, "space->is_references > 0");
    (*space).is_references -= 1;
    let refs = (*space).is_references;
    /* simple_unlock(...) — no-op */
    if refs == 0 {
        is_free(space);
    }
}

#[inline]
unsafe fn is_reference_macro(space: ipc_space_t) {
    /* simple_lock — no-op */
    crate::kassert!((*space).is_references > 0, "space->is_references > 0");
    (*space).is_references += 1;
    /* simple_unlock — no-op */
}

// ---------------------------------------------------------------------------
//  ipc_space_reference / ipc_space_release  — function versions of the macros
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_space_reference(space: ipc_space_t) {
    is_reference_macro(space);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_space_release(space: ipc_space_t) {
    is_release_macro(space);
}

// ---------------------------------------------------------------------------
//  ipc_space_create
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_space_create(
    spacep: *mut ipc_space_t,
) -> kern_return_t {
    let space = is_alloc();
    if space == IS_NULL {
        return KERN_RESOURCE_SHORTAGE;
    }

    is_ref_lock_init(space);
    (*space).is_references = 2;

    is_lock_init(space);
    (*space).is_active = 1; /* TRUE */

    rdxtree_init(addr_of_mut!((*space).is_map));
    rdxtree_init(addr_of_mut!((*space).is_reverse_map));
    /* The zeroth entry is reserved.  */
    rdxtree_insert_common(
        addr_of_mut!((*space).is_map),
        0,
        addr_of_mut!(zero_entry) as *mut core::ffi::c_void,
        null_mut(),
    );
    (*space).is_size = 1;
    (*space).is_free_list = core::ptr::null_mut();
    (*space).is_free_list_size = 0;

    *spacep = space;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_space_create_special
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_space_create_special(
    spacep: *mut ipc_space_t,
) -> kern_return_t {
    let space = is_alloc();
    if space == IS_NULL {
        return KERN_RESOURCE_SHORTAGE;
    }

    is_ref_lock_init(space);
    (*space).is_references = 1;

    is_lock_init(space);
    (*space).is_active = 0; /* FALSE */

    *spacep = space;
    KERN_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_space_destroy
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_space_destroy(space: ipc_space_t) {
    crate::kassert!(space != IS_NULL, "space != IS_NULL");

    is_write_lock(space);
    let active = (*space).is_active;
    (*space).is_active = 0; /* FALSE */
    is_write_unlock(space);

    if active == 0 {
        return;
    }

    /* rdxtree_for_each(&space->is_map, &iter, entry) { ... } */
    let mut iter = rdxtree_iter { node: null_mut(), key: !0 };
    rdxtree_iter_init(&mut iter);
    let mut entry = rdxtree_walk(addr_of_mut!((*space).is_map), &mut iter)
        as ipc_entry_t;
    while !entry.is_null() {
        if (*entry).ie_name != MACH_PORT_NAME_NULL {
            let typ = ie_bits_type((*entry).ie_bits);
            if typ != 0 {
                let name = mach_port_makeb(
                    (*entry).ie_name,
                    (*entry).ie_bits,
                );
                ipc_right_clean(space, name, entry);
            }
            ie_free(entry);
        }
        entry = rdxtree_walk(addr_of_mut!((*space).is_map), &mut iter)
            as ipc_entry_t;
    }
    rdxtree_remove_all(addr_of_mut!((*space).is_map));
    rdxtree_remove_all(addr_of_mut!((*space).is_reverse_map));

    /*
     *  Because the space is now dead,
     *  we must release the "active" reference for it.
     *  Our caller still has his reference.
     */
    is_release_macro(space);
}

#[allow(dead_code)]
fn _silence_rdxtree_key_t(_k: rdxtree_key_t) {}
