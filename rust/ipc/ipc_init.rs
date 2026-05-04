//! Port of `ipc/ipc_init.c`.
//!
//! Bootstrap and final init for the IPC subsystem.

use core::ptr::addr_of_mut;

use crate::extern_c::{
    ipc_host_init, ipc_marequest_init,
    kernel_map, kmem_cache_init, kmem_submap, Assert,
};
use crate::ipc_entry::ipc_entry_cache;
use crate::ipc_notify::ipc_notify_init;
use crate::ipc_object::ipc_object_caches;
use crate::ipc_port::ipc_port_timestamp_data;
use crate::ipc_space::{
    ipc_space_cache, ipc_space_create_special, ipc_space_kernel,
    ipc_space_reply,
};
use crate::ipc_table::ipc_table_init;
use crate::mach_types::{
    vm_map_t, vm_offset_t, vm_size_t, IOT_PORT, IOT_PORT_SET, KERN_SUCCESS,
    SIZE_OF_IPC_ENTRY, SIZE_OF_IPC_PORT, SIZE_OF_IPC_PSET, SIZE_OF_IPC_SPACE,
    SIZE_OF_VM_MAP,
};

// `static struct vm_map ipc_kernel_map_store;`
//
// Rust never accesses fields of vm_map, so model the storage as an opaque
// blob with alignment 4 and size SIZE_OF_VM_MAP (= 84, pinned by
// _Static_assert in ipc/ipc_layout_asserts.c).
#[repr(C, align(4))]
struct VmMapStorage([u8; SIZE_OF_VM_MAP]);

static mut ipc_kernel_map_store: VmMapStorage =
    VmMapStorage([0; SIZE_OF_VM_MAP]);

/// Global: `vm_map_t ipc_kernel_map = &ipc_kernel_map_store;`
#[no_mangle]
pub static mut ipc_kernel_map: vm_map_t =
    unsafe { addr_of_mut!(ipc_kernel_map_store) as vm_map_t };

/// Global: `const vm_size_t ipc_kernel_map_size = 8 * 1024 * 1024;`
#[no_mangle]
pub static ipc_kernel_map_size: vm_size_t = 8 * 1024 * 1024;

/// `ipc_bootstrap` — initialization needed before the kernel task can be
/// created.
#[no_mangle]
pub unsafe extern "C" fn ipc_bootstrap() {
    let kr;

    // ipc_port_multiple_lock_init() and ipc_port_timestamp_lock_init() expand
    // (with NCPUS == 1, MACH_SLOCKS == 0) to simple_lock_assert(...) which is
    // a (void)-cast type-check macro with no runtime effect.

    ipc_port_timestamp_data = 0;

    kmem_cache_init(
        addr_of_mut!(ipc_space_cache),
        b"ipc_space\0".as_ptr() as *const _,
        SIZE_OF_IPC_SPACE,
        0,
        None,
        0,
    );

    kmem_cache_init(
        addr_of_mut!(ipc_entry_cache),
        b"ipc_entry\0".as_ptr() as *const _,
        SIZE_OF_IPC_ENTRY,
        0,
        None,
        0,
    );

    kmem_cache_init(
        addr_of_mut!(ipc_object_caches[IOT_PORT as usize]),
        b"ipc_port\0".as_ptr() as *const _,
        SIZE_OF_IPC_PORT,
        0,
        None,
        0,
    );

    kmem_cache_init(
        addr_of_mut!(ipc_object_caches[IOT_PORT_SET as usize]),
        b"ipc_pset\0".as_ptr() as *const _,
        SIZE_OF_IPC_PSET,
        0,
        None,
        0,
    );

    /* create special spaces */

    kr = ipc_space_create_special(addr_of_mut!(ipc_space_kernel));
    if kr != KERN_SUCCESS {
        Assert(
            b"rust/ipc/ipc_init.rs\0".as_ptr(),
            0,
            b"ipc_bootstrap\0".as_ptr(),
            b"kr == KERN_SUCCESS\0".as_ptr(),
        );
    }

    let kr = ipc_space_create_special(addr_of_mut!(ipc_space_reply));
    if kr != KERN_SUCCESS {
        Assert(
            b"rust/ipc/ipc_init.rs\0".as_ptr(),
            0,
            b"ipc_bootstrap\0".as_ptr(),
            b"kr == KERN_SUCCESS\0".as_ptr(),
        );
    }

    /* initialize modules with hidden data structures */

    ipc_table_init();
    ipc_notify_init();
    ipc_marequest_init();
}

/// `ipc_init` — final initialization of the IPC system.
#[no_mangle]
pub unsafe extern "C" fn ipc_init() {
    let mut min: vm_offset_t = 0;
    let mut max: vm_offset_t = 0;

    kmem_submap(
        ipc_kernel_map,
        kernel_map,
        addr_of_mut!(min),
        addr_of_mut!(max),
        ipc_kernel_map_size,
    );

    ipc_host_init();
}

// Suppress unused-warning for `min`/`max` (kmem_submap writes through the
// pointers but Rust's flow analysis can't see across the FFI call).
#[allow(dead_code)]
fn _used(_a: vm_offset_t, _b: vm_offset_t) {}
