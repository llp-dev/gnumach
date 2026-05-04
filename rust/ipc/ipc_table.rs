//! Port of `ipc/ipc_table.c`.
//!
//! Tables of IPC capabilities (sizing helpers + alloc/free wrappers).

use core::mem::size_of;

use crate::extern_c::{kalloc, kfree, Assert};
use crate::mach_types::{
    ipc_port_request, ipc_table_size, ipc_table_size_t, vm_offset_t, vm_size_t,
    ITS_NULL, PAGE_SIZE,
};

/// Global: `ipc_table_size_t ipc_table_dnrequests;`
#[no_mangle]
pub static mut ipc_table_dnrequests: ipc_table_size_t = ITS_NULL;

/// Global: `const unsigned int ipc_table_dnrequests_size = 64;`
#[no_mangle]
pub static ipc_table_dnrequests_size: u32 = 64;

/// `ipc_table_fill` — fill `its[0..num]` with progressively larger sizes,
/// each at least `min` elements, where each entry's `its_size` × `elemsize`
/// approximates a power of two up to `PAGE_SIZE`, then increments by
/// successively larger page multiples.
#[no_mangle]
pub unsafe extern "C" fn ipc_table_fill(
    its: ipc_table_size_t, /* array to fill */
    num: u32,              /* size of array */
    min: u32,              /* at least this many elements */
    elemsize: vm_size_t,   /* size of elements */
) {
    let mut index: u32;
    let minsize: vm_size_t = min * elemsize;
    let mut size: vm_size_t;
    let mut incrsize: vm_size_t;

    /* first use powers of two, up to the page size */
    index = 0;
    size = 1;
    while index < num && size < PAGE_SIZE {
        if size >= minsize {
            (*its.add(index as usize)).its_size = size / elemsize;
            index += 1;
        }
        size <<= 1;
    }

    /* then increments of a page, then two pages, etc. */
    incrsize = PAGE_SIZE;
    while index < num {
        let mut period: u32 = 0;
        while period < 15 && index < num {
            if size >= minsize {
                (*its.add(index as usize)).its_size = size / elemsize;
                index += 1;
            }
            period += 1;
            size += incrsize;
        }
        if incrsize < (PAGE_SIZE << 3) {
            incrsize <<= 1;
        }
    }
}

/// `ipc_table_init` — allocate `ipc_table_dnrequests`, populate it, and
/// terminate with a zero-sized sentinel.
#[no_mangle]
pub unsafe extern "C" fn ipc_table_init() {
    ipc_table_dnrequests = kalloc(
        size_of::<ipc_table_size>() as vm_size_t * ipc_table_dnrequests_size,
    ) as ipc_table_size_t;
    if ipc_table_dnrequests == ITS_NULL {
        Assert(
            b"rust/ipc/ipc_table.rs\0".as_ptr(),
            0,
            b"ipc_table_init\0".as_ptr(),
            b"ipc_table_dnrequests != ITS_NULL\0".as_ptr(),
        );
    }

    ipc_table_fill(
        ipc_table_dnrequests,
        ipc_table_dnrequests_size - 1,
        2,
        size_of::<ipc_port_request>() as vm_size_t,
    );

    /* the last element should have zero size */
    (*ipc_table_dnrequests.add((ipc_table_dnrequests_size - 1) as usize))
        .its_size = 0;
}

/// `ipc_table_alloc` — allocate a table.  May block.
#[no_mangle]
pub unsafe extern "C" fn ipc_table_alloc(size: vm_size_t) -> vm_offset_t {
    kalloc(size)
}

/// `ipc_table_free` — free a table allocated with [`ipc_table_alloc`].
/// May block.
#[no_mangle]
pub unsafe extern "C" fn ipc_table_free(size: vm_size_t, table: vm_offset_t) {
    kfree(table, size);
}
