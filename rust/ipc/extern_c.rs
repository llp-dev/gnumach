//! `extern "C"` declarations of kernel C symbols the Rust side calls.

use core::ffi::{c_char, c_int};

use crate::mach_types::{
    boolean_t, ipc_entry_t, ipc_kmsg_full, ipc_mqueue, ipc_object, ipc_port,
    ipc_port_timestamp_t, ipc_space, ipc_space_t, kern_return_t, kmem_cache,
    lock_t, mach_msg_option_t, mach_msg_return_t, mach_msg_timeout_t,
    mach_port_name_t, rdxtree, rdxtree_key_t, vm_map_t, vm_offset_t,
    vm_size_t,
};

/// Type of kmem_cache constructor: `void (*)(void *)`.
pub type kmem_cache_ctor_t =
    Option<unsafe extern "C" fn(*mut core::ffi::c_void)>;

extern "C" {
    /// `kern/kalloc.h`: `vm_offset_t kalloc(vm_size_t);`
    pub fn kalloc(size: vm_size_t) -> vm_offset_t;
    /// `kern/kalloc.h`: `void kfree(vm_offset_t addr, vm_size_t size);`
    pub fn kfree(addr: vm_offset_t, size: vm_size_t);

    /// `kern/slab.h`:
    /// `void kmem_cache_init(struct kmem_cache *cache, const char *name,
    ///                       size_t obj_size, size_t align,
    ///                       kmem_cache_ctor_t ctor, int flags);`
    pub fn kmem_cache_init(
        cache: *mut kmem_cache,
        name: *const c_char,
        obj_size: usize,
        align: usize,
        ctor: kmem_cache_ctor_t,
        flags: c_int,
    );

    /// `kern/debug.h`:
    /// `void Assert(const char *file, int line, const char *fun, const char *s, ...);`
    pub fn Assert(
        file: *const u8,
        line: c_int,
        fun: *const u8,
        s: *const u8,
    ) -> !;


    // Other init routines that remain in C for now.
    pub fn ipc_marequest_init();

    // VM.
    pub static mut kernel_map: vm_map_t;
    pub fn kmem_submap(
        new_map: vm_map_t,
        parent: vm_map_t,
        min: *mut vm_offset_t,
        max: *mut vm_offset_t,
        size: vm_size_t,
    );

    // Host.
    pub fn ipc_host_init();

    // printf for the "dropped" log lines in ipc_notify.
    pub fn printf(fmt: *const c_char, ...) -> c_int;

    // ipc_marequest dependencies.
    /// `kern/slab.h`: `vm_offset_t kmem_cache_alloc(struct kmem_cache *cache);`
    pub fn kmem_cache_alloc(cache: *mut kmem_cache) -> vm_offset_t;
    /// `kern/slab.h`: `void kmem_cache_free(struct kmem_cache *cache, vm_offset_t obj);`
    pub fn kmem_cache_free(cache: *mut kmem_cache, obj: vm_offset_t);

    /// `kern/lock.h`: `void lock_write(lock_t);`
    pub fn lock_write(lock: lock_t);
    /// `kern/lock.h`: `void lock_read(lock_t);`
    pub fn lock_read(lock: lock_t);
    /// `kern/lock.h`: `void lock_done(lock_t);`
    pub fn lock_done(lock: lock_t);

    /// `kern/rdxtree.h`: looks up a key, returning the stored pointer or NULL.
    pub fn rdxtree_lookup_common(
        tree: *const rdxtree,
        key: rdxtree_key_t,
        slot: c_int,
    ) -> *mut core::ffi::c_void;

    /// `kern/rdxtree.c`: insert with explicit key (returns 0 on success).
    pub fn rdxtree_insert_common(
        tree: *mut rdxtree,
        key: rdxtree_key_t,
        ptr: *mut core::ffi::c_void,
        slotp: *mut *mut *mut core::ffi::c_void,
    ) -> c_int;

    /// `kern/rdxtree.c`: insert and allocate a key (returns 0 on success).
    pub fn rdxtree_insert_alloc_common(
        tree: *mut rdxtree,
        ptr: *mut core::ffi::c_void,
        keyp: *mut rdxtree_key_t,
        slotp: *mut *mut *mut core::ffi::c_void,
    ) -> c_int;

    /// `kern/rdxtree.h`: replace the pointer in a slot, returning the prior value.
    pub fn rdxtree_replace_slot(
        slot: *mut *mut core::ffi::c_void,
        ptr: *mut core::ffi::c_void,
    ) -> *mut core::ffi::c_void;

    /// `kern/rdxtree.h`: remove a key, returning the prior pointer or NULL.
    pub fn rdxtree_remove(
        tree: *mut rdxtree,
        key: rdxtree_key_t,
    ) -> *mut core::ffi::c_void;

    /// `kern/sched_prim.h`: `void thread_go(thread_t);`
    pub fn thread_go(thread: crate::mach_types::ipc_thread_t);

    /// `kern/sched_prim.h`: `void thread_will_wait(thread_t);`
    pub fn thread_will_wait(thread: crate::mach_types::ipc_thread_t);

    /// `kern/sched_prim.h`:
    /// `void thread_will_wait_with_timeout(thread_t, mach_msg_timeout_t);`
    pub fn thread_will_wait_with_timeout(
        thread: crate::mach_types::ipc_thread_t,
        msecs: mach_msg_timeout_t,
    );

    /// `kern/sched_prim.h`: `void thread_block(continuation_t);`
    pub fn thread_block(continuation: crate::mach_types::continuation_t);

    /// `kern/ipc_kobject.h`:
    /// `ipc_kmsg_t ipc_kobject_server(ipc_kmsg_t);`
    pub fn ipc_kobject_server(
        kmsg: *mut crate::mach_types::ipc_kmsg_full,
    ) -> *mut crate::mach_types::ipc_kmsg_full;

    /// Base of the per-CPU array.  Only the address is used; the actual
    /// `struct percpu` is several hundred bytes.
    pub static mut percpu_array: u8;

    /// `kern/ipc_kobject.h`: `void ipc_kobject_destroy(ipc_port_t);`
    pub fn ipc_kobject_destroy(port: *mut ipc_port);


    /// `vm/vm_kern.h`: `kern_return_t kmem_alloc_pageable(vm_map_t, vm_offset_t *, vm_size_t);`
    pub fn kmem_alloc_pageable(
        map: vm_map_t,
        addrp: *mut vm_offset_t,
        size: vm_size_t,
    ) -> kern_return_t;

    /// `vm/vm_kern.h`: `void kmem_free(vm_map_t, vm_offset_t, vm_size_t);`
    pub fn kmem_free(map: vm_map_t, addr: vm_offset_t, size: vm_size_t);

    /// `vm/vm_user.h`: `kern_return_t vm_map_copyin(vm_map_t, vm_offset_t,
    ///     vm_size_t, boolean_t src_destroy, vm_map_copy_t *);`
    pub fn vm_map_copyin(
        src_map: vm_map_t,
        src_addr: vm_offset_t,
        len: vm_size_t,
        src_destroy: boolean_t,
        copy_result: *mut crate::mach_types::vm_map_copy_t,
    ) -> kern_return_t;

    /// `vm/vm_map.h`: `void vm_map_copy_discard(vm_map_copy_t);`
    pub fn vm_map_copy_discard(copy: crate::mach_types::vm_map_copy_t);

    /// `vm/vm_map.h`: `kern_return_t vm_map_copyin_page_list(vm_map_t,
    ///     vm_offset_t, vm_size_t, boolean_t src_destroy, boolean_t steal_pages,
    ///     vm_map_copy_t *result, boolean_t cont);`
    pub fn vm_map_copyin_page_list(
        src_map: vm_map_t,
        src_addr: vm_offset_t,
        len: vm_size_t,
        src_destroy: boolean_t,
        steal_pages: boolean_t,
        copy_result: *mut crate::mach_types::vm_map_copy_t,
        cont: boolean_t,
    ) -> kern_return_t;

    /// `vm/vm_user.h`:
    /// `kern_return_t vm_deallocate(vm_map_t, vm_offset_t, vm_size_t);`
    pub fn vm_deallocate(
        map: vm_map_t,
        start: vm_offset_t,
        size: vm_size_t,
    ) -> kern_return_t;

    /// `vm/vm_kern.h`: `int copyinmap(vm_map_t, char *, char *, int);`
    pub fn copyinmap(
        map: vm_map_t,
        fromaddr: *const c_char,
        toaddr: *mut c_char,
        len: c_int,
    ) -> c_int;

    /// `vm/vm_kern.h`: `int copyoutmap(vm_map_t, char *, char *, int);`
    pub fn copyoutmap(
        map: vm_map_t,
        fromaddr: *const c_char,
        toaddr: *mut c_char,
        len: c_int,
    ) -> c_int;

    /// `vm/vm_map.h`:
    /// `kern_return_t vm_map_copyout(vm_map_t, vm_offset_t *, vm_map_copy_t);`
    pub fn vm_map_copyout(
        dst_map: vm_map_t,
        dst_addr: *mut vm_offset_t,
        copy: crate::mach_types::vm_map_copy_t,
    ) -> kern_return_t;


    /// `i386/i386/copy_user.S`: low-level user→kernel single-port copy.
    /// (Inline `copyin_port` in `ipc/copy_user.h` calls plain `copyin`.)
    pub fn copyin(
        userbuf: *const core::ffi::c_void,
        kernelbuf: *mut core::ffi::c_void,
        nbytes: usize,
    ) -> c_int;

    /// `vm/vm_user.h`: `kern_return_t vm_allocate(vm_map_t, vm_offset_t *, vm_size_t, boolean_t);`
    pub fn vm_allocate(
        map: vm_map_t,
        addrp: *mut vm_offset_t,
        size: vm_size_t,
        anywhere: boolean_t,
    ) -> kern_return_t;

    /// `vm/vm_map.h`: `kern_return_t vm_map_pageable(vm_map_t, vm_offset_t,
    ///     vm_offset_t, vm_prot_t, boolean_t, boolean_t);`
    pub fn vm_map_pageable(
        map: vm_map_t,
        start: vm_offset_t,
        end: vm_offset_t,
        access_type: crate::mach_types::vm_prot_t,
        user_wire: boolean_t,
        lock_map: boolean_t,
    ) -> kern_return_t;

    /// `kern/ipc_kobject.h`: `void ipc_kobject_set_locked(ipc_port_t,
    ///     ipc_kobject_t, ipc_kobject_type_t);`
    pub fn ipc_kobject_set_locked(
        port: *mut ipc_port,
        kobject: crate::mach_types::ipc_kobject_t,
        kotype: u32,
    );

    /// Diagnostic printf-once.  Implemented C-side as a printf with a flag.
    pub fn printf_once(fmt: *const c_char, ...) -> c_int;

    /// `kern/debug.h`: `void SoftDebugger(const char *msg);`
    pub fn SoftDebugger(msg: *const c_char);

    /// `kern/thread.h`: `void thread_syscall_return(kern_return_t)` — does not return.
    pub fn thread_syscall_return(ret: kern_return_t) -> !;

    /// `kern/thread.h`: `void thread_set_syscall_return(thread_t, kern_return_t);`
    pub fn thread_set_syscall_return(
        thread: crate::mach_types::ipc_thread_t,
        ret: kern_return_t,
    );

    /// `kern/thread.h`: `void thread_exception_return(void);`  (Continuation.)
    pub fn thread_exception_return() -> !;

    /// `string.h`: `int copyout(const void *kaddr, void *uaddr, size_t len);`
    pub fn copyout(
        kaddr: *const core::ffi::c_void,
        uaddr: *mut core::ffi::c_void,
        len: usize,
    ) -> c_int;

    /// `i386/i386/locore.h`:
    /// `int copyinmsg(const void *userbuf, void *kernelbuf, size_t cn, size_t kn);`
    pub fn copyinmsg(
        userbuf: *const core::ffi::c_void,
        kernelbuf: *mut core::ffi::c_void,
        cn: usize,
        kn: usize,
    ) -> c_int;

    /// `i386/i386/locore.S`:
    /// `int copyoutmsg(const void *kaddr, void *uaddr, size_t count);`
    pub fn copyoutmsg(
        kaddr: *const core::ffi::c_void,
        uaddr: *mut core::ffi::c_void,
        count: usize,
    ) -> c_int;








    /// `kern/lock.h`: `void lock_init(lock_t, boolean_t);`
    pub fn lock_init(lock: lock_t, can_sleep: crate::mach_types::boolean_t);

    /// `kern/rdxtree.h`: walk one step (returns next stored pointer or NULL).
    pub fn rdxtree_walk(
        tree: *mut rdxtree,
        iter: *mut crate::mach_types::rdxtree_iter,
    ) -> *mut core::ffi::c_void;

    /// `kern/rdxtree.h`: drop every node in the tree.
    pub fn rdxtree_remove_all(tree: *mut rdxtree);

}
