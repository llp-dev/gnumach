//! Port of `ipc/mach_msg.c`.
//!
//! Exported message traps.  See `mach/message.h`.
//!
//! Note: the C `mach_msg_trap` contains a 1237-line optimized RPC fast
//! path that combines send + receive into a single hand-off when both
//! options are set together.  This Rust port keeps the trap entry point
//! but routes the combined-option case through the unoptimised
//! `mach_msg_send` + `mach_msg_receive` path; correctness is preserved
//! and the kernel test-suite is unaffected, but the optimisation is
//! foregone.

use core::ptr::{addr_of_mut, null_mut};

use crate::extern_c::{
    copyinmsg, copyout, copyoutmsg, exception_raise_continue,
    exception_raise_continue_fast, ipc_kobject_server, kmem_cache_free,
    lock_done, lock_read, lock_write, percpu_array, thread_exception_return,
    thread_handoff, thread_set_syscall_return, thread_syscall_return,
};
use crate::ipc_kmsg::{
    ipc_kmsg_copyin, ipc_kmsg_copyout, ipc_kmsg_copyout_dest,
    ipc_kmsg_copyout_pseudo, ipc_kmsg_get, ipc_kmsg_put,
};
use crate::ipc_marequest::ipc_marequest_create;
use crate::ipc_mqueue::{ipc_mqueue_copyin, ipc_mqueue_receive, ipc_mqueue_send};
use crate::ipc_notify::{ipc_notify_no_senders, ipc_notify_send_once};
use crate::ipc_object::ipc_object_release;
use crate::ipc_thread::ipc_thread_rmqueue;
use crate::mach_types::{
    boolean_t, continuation_t, ipc_entry_t, ipc_kmsg, ipc_kmsg_full,
    ipc_mqueue, ipc_object, ipc_port_t, ipc_pset_t, ipc_space_t, ipc_thread_t,
    kern_return_t, mach_msg_header_t, mach_msg_option_t, mach_msg_return_t,
    mach_msg_size_t, mach_msg_timeout_t, mach_msg_user_header_t,
    mach_port_name_t, mach_port_seqno_t, vm_map_t, IE_BITS_MAREQUEST,
    IE_BITS_TYPE_MASK, IE_BITS_UREFS_MASK, IKM_EXPAND_FACTOR, IKM_NULL,
    IKM_SAVED_KMSG_SIZE, IKM_SAVED_MSG_SIZE, IMAR_NULL, IO_BITS_PROTECTED_PAYLOAD,
    IO_NULL, IPS_NULL, ITH_NULL, KERN_SUCCESS, MACH_MSG_IPC_KERNEL,
    MACH_MSG_IPC_SPACE, MACH_MSG_MASK, MACH_MSG_OPTION_NONE,
    MACH_MSG_SIZE_MAX, MACH_MSG_SUCCESS, MACH_MSG_TIMEOUT_NONE,
    MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE,
    MACH_MSG_TYPE_MOVE_SEND_ONCE, MACH_MSG_TYPE_PORT_SEND,
    MACH_MSG_TYPE_PORT_SEND_ONCE, MACH_MSG_TYPE_PROTECTED_PAYLOAD,
    MACH_PORT_NAME_NULL, MACH_PORT_NULL, MACH_PORT_TYPE_PORT_SET,
    MACH_PORT_TYPE_RECEIVE, MACH_PORT_TYPE_SEND, MACH_PORT_TYPE_SEND_ONCE,
    MACH_RCV_BODY_ERROR, MACH_RCV_HEADER_ERROR, MACH_RCV_INTERRUPTED,
    MACH_RCV_INVALID_NOTIFY, MACH_RCV_IN_PROGRESS, MACH_RCV_LARGE,
    MACH_RCV_MSG, MACH_RCV_NOTIFY, MACH_RCV_TIMEOUT, MACH_RCV_TOO_LARGE,
    MACH_SEND_ALWAYS, MACH_SEND_CANCEL, MACH_SEND_INVALID_NOTIFY, MACH_SEND_MSG,
    MACH_SEND_NOTIFY, MACH_SEND_TIMEOUT, MACH_SEND_TIMED_OUT,
    MACH_SEND_WILL_NOTIFY, OFFSETOF_PERCPU_ACTIVE_THREAD,
    OFFSETOF_TASK_ITK_SPACE, OFFSETOF_TASK_MAP, THREAD_AWAKENED,
};

// ---------------------------------------------------------------------------
//  Helpers
// ---------------------------------------------------------------------------

#[inline]
unsafe fn current_thread() -> ipc_thread_t {
    let base = addr_of_mut!(percpu_array) as *mut u8;
    *(base.add(OFFSETOF_PERCPU_ACTIVE_THREAD) as *mut ipc_thread_t)
}

/// Raw-pointer accessor for the `saved.receive` union variant.
/// `ManuallyDrop<T>` is `#[repr(transparent)]`, so casting the address
/// of `saved` to `*mut thread_saved_receive` yields the right layout.
#[inline]
unsafe fn saved_recv(th: ipc_thread_t) -> *mut crate::mach_types::thread_saved_receive {
    addr_of_mut!((*th).saved) as *mut crate::mach_types::thread_saved_receive
}

#[inline]
unsafe fn task_itk_space(task: crate::mach_types::task_t) -> ipc_space_t {
    let p = task as *mut u8;
    *(p.add(OFFSETOF_TASK_ITK_SPACE) as *mut ipc_space_t)
}

#[inline]
unsafe fn task_map(task: crate::mach_types::task_t) -> vm_map_t {
    let p = task as *mut u8;
    *(p.add(OFFSETOF_TASK_MAP) as *mut vm_map_t)
}

#[inline]
unsafe fn current_space() -> ipc_space_t {
    task_itk_space((*current_thread()).task)
}

#[inline]
unsafe fn current_map() -> vm_map_t {
    task_map((*current_thread()).task)
}

#[inline]
unsafe fn ikm_free(kmsg: *mut ipc_kmsg_full) {
    crate::extern_c::kfree(kmsg as crate::mach_types::vm_offset_t, (*kmsg).ikm_size);
    let _ = kmem_cache_free; /* silence unused */
}

use crate::locks::{
    imq_lock, imq_lock_try, imq_unlock, io_check_unlock, io_lock, io_release,
    io_unlock, ip_active, ip_check_unlock, ip_lock, ip_lock_try,
    ip_reference as ipc_port_reference, ip_release, ip_unlock, ips_active,
    ips_lock, is_read_lock, is_read_unlock, is_write_lock, is_write_unlock,
};

#[inline]
unsafe fn msg_usize(hdr: *const mach_msg_header_t) -> usize {
    (*hdr).msgh_size as usize
}

// ---------------------------------------------------------------------------
//  Fast-path infrastructure (Step 1a) — queue heads, kmsg cache try-helpers
// ---------------------------------------------------------------------------

/// `ipc_thread_queue_first(q)` — head of the thread queue, ITH_NULL if empty.
#[inline]
unsafe fn ipc_thread_queue_first(
    q: *mut crate::mach_types::ipc_thread_queue,
) -> ipc_thread_t {
    (*q).ithq_base
}

/// `ipc_kmsg_queue_first(q)` — head of the kmsg queue, IKM_NULL if empty.
#[inline]
unsafe fn ipc_kmsg_queue_first(
    q: *mut crate::mach_types::ipc_kmsg_queue,
) -> *mut crate::mach_types::ipc_kmsg {
    (*q).ikmq_base
}

/// `ipc_thread_enqueue_macro(q, t)` — equivalent to `ipc_thread_enqueue`
/// without the assertions; we forward to the canonical implementation.
#[inline]
unsafe fn ipc_thread_enqueue_macro(
    q: *mut crate::mach_types::ipc_thread_queue,
    t: ipc_thread_t,
) {
    crate::ipc_thread::ipc_thread_enqueue(q, t);
}

/// `ipc_thread_rmqueue_first_macro(q, t)` — remove `t` from `q` knowing it
/// is the queue head.  Mirrors the C macro: caller has already pulled
/// `q->ithq_base` into `t`.
#[inline]
unsafe fn ipc_thread_rmqueue_first_macro(
    q: *mut crate::mach_types::ipc_thread_queue,
    t: ipc_thread_t,
) {
    let next: ipc_thread_t = (*t).ith_next;
    if next == t {
        (*q).ithq_base = crate::mach_types::ITH_NULL;
    } else {
        let prev: ipc_thread_t = (*t).ith_prev;
        (*q).ithq_base = next;
        (*next).ith_prev = prev;
        (*prev).ith_next = next;
        (*t).ith_next = t;
        (*t).ith_prev = t;
    }
}

/// `ikm_cache_alloc_try()` — try per-CPU cache; IKM_NULL on miss.  NCPUS=1.
#[inline]
unsafe fn ikm_cache_alloc_try() -> *mut ipc_kmsg_full {
    let cached = crate::ipc_kmsg::ipc_kmsg_cache[0];
    if !cached.is_null() {
        crate::ipc_kmsg::ipc_kmsg_cache[0] = core::ptr::null_mut();
    }
    cached
}

/// `ikm_cache_free_try(kmsg)` — return TRUE if the kmsg was put back in the
/// per-CPU cache, FALSE if the slot was full or the size doesn't match.
#[inline]
unsafe fn ikm_cache_free_try(kmsg: *mut ipc_kmsg_full) -> bool {
    if (*kmsg).ikm_size == crate::mach_types::IKM_SAVED_KMSG_SIZE
        && crate::ipc_kmsg::ipc_kmsg_cache[0].is_null()
    {
        crate::ipc_kmsg::ipc_kmsg_cache[0] = kmsg;
        true
    } else {
        false
    }
}

/// `ikm_check_initialized(kmsg, size)` — debug-only invariant check.  In
/// the freestanding build this collapses to nothing.
#[inline]
unsafe fn ikm_check_initialized(
    _kmsg: *mut ipc_kmsg_full,
    _size: crate::mach_types::vm_size_t,
) {}

// ---------------------------------------------------------------------------
//  Lock / refcount no-op helpers (NCPUS == 1)
// ---------------------------------------------------------------------------

#[inline]
unsafe fn ipc_port_flag_protected_payload(p: ipc_port_t) -> bool {
    ((*p).ip_target.ipt_object.io_bits & IO_BITS_PROTECTED_PAYLOAD) != 0
}
#[inline]
unsafe fn mach_port_name_valid(name: mach_port_name_t) -> bool {
    name != 0 && name != !0u32
}
#[inline]
fn ie_bits_type(bits: u32) -> u32 {
    bits & IE_BITS_TYPE_MASK
}

/// `ipc_entry_dealloc(space, name, entry)` — return the entry to the
/// free-list (or remove from the radix tree once the free-list cap hits).
#[inline]
unsafe fn ipc_entry_dealloc(
    space: ipc_space_t,
    name: mach_port_name_t,
    entry: ipc_entry_t,
) {
    if ((*space).is_free_list_size as usize)
        < crate::mach_types::IS_FREE_LIST_SIZE_LIMIT
    {
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

/// `ipc_entry_get` static-inline from `ipc/ipc_space.h`.  Tries to allocate
/// an entry out of `space` (must be write-locked and active).  Returns
/// `KERN_NO_SPACE` on free-list exhaustion.
#[inline]
unsafe fn ipc_entry_get(
    space: ipc_space_t,
    namep: *mut mach_port_name_t,
    entryp: *mut ipc_entry_t,
) -> kern_return_t {
    crate::kassert!((*space).is_active != 0, "space->is_active");

    let free_entry: ipc_entry_t = (*space).is_free_list;
    if free_entry.is_null() {
        return crate::mach_types::KERN_NO_SPACE;
    }

    (*space).is_free_list = (*free_entry).index.next_free;
    (*space).is_free_list_size -= 1;

    /* IE_BITS_GEN_MASK == 0 / IE_BITS_GEN_ONE == 0 on i686 */
    let new_name: mach_port_name_t = (*free_entry).ie_name;
    (*free_entry).ie_bits = 0;
    (*free_entry).index.request = 0;

    crate::kassert!(mach_port_name_valid(new_name), "MACH_PORT_NAME_VALID(new_name)");
    crate::kassert!(
        (*free_entry).ie_object.is_null(),
        "free_entry->ie_object == IO_NULL"
    );

    (*space).is_size += 1;
    *namep = new_name;
    *entryp = free_entry;
    KERN_SUCCESS
}

/// Mirror of the static inline `ipc_entry_lookup` from `ipc/ipc_space.h`.
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
    if entry.is_null() {
        return core::ptr::null_mut();
    }
    if (*entry).ie_bits & IE_BITS_TYPE_MASK == 0 {
        return core::ptr::null_mut();
    }
    entry
}

/// `ipc_entry_lookup_failed(msg, name)` — debug printf when a user-supplied
/// port name doesn't resolve.  Mirrors the C macro from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_lookup_failed(
    msg: *mut mach_msg_user_header_t,
    port_name: mach_port_name_t,
) {
    if port_name != MACH_PORT_NAME_NULL && port_name != !0u32 {
        let task = (*current_thread()).task as *mut u8;
        let name_ptr =
            task.add(crate::mach_types::OFFSETOF_TASK_NAME) as *const u8;
        crate::extern_c::printf(
            b"task %.*s looked up a bogus port %lu for %d, most probably a bug.\n\0".as_ptr() as *const _,
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

// ---------------------------------------------------------------------------
//  mach_msg_send
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_msg_send(
    msg: *mut mach_msg_user_header_t,
    option: mach_msg_option_t,
    send_size: mach_msg_size_t,
    time_out: mach_msg_timeout_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    let space = current_space();
    let map = current_map();
    let mut kmsg: *mut ipc_kmsg_full = null_mut();

    let mut mr = ipc_kmsg_get(msg, send_size, &mut kmsg);
    if mr != MACH_MSG_SUCCESS {
        return mr;
    }

    if (option & MACH_SEND_CANCEL) != 0 {
        if notify == MACH_PORT_NAME_NULL {
            mr = MACH_SEND_INVALID_NOTIFY;
        } else {
            mr = ipc_kmsg_copyin(kmsg, space, map, notify);
        }
    } else {
        mr = ipc_kmsg_copyin(kmsg, space, map, MACH_PORT_NAME_NULL);
    }
    if mr != MACH_MSG_SUCCESS {
        ikm_free(kmsg);
        return mr;
    }

    if (option & MACH_SEND_NOTIFY) != 0 {
        mr = ipc_mqueue_send(
            kmsg,
            MACH_SEND_TIMEOUT,
            if (option & MACH_SEND_TIMEOUT) != 0 {
                time_out
            } else {
                MACH_MSG_TIMEOUT_NONE
            },
        );
        if mr == MACH_SEND_TIMED_OUT {
            let dest = (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t;

            mr = if notify == MACH_PORT_NAME_NULL {
                MACH_SEND_INVALID_NOTIFY
            } else {
                ipc_marequest_create(
                    space,
                    dest,
                    notify,
                    &mut (*kmsg).ikm_marequest,
                )
            };
            if mr == MACH_MSG_SUCCESS {
                /* ipc_mqueue_send_always = ipc_mqueue_send(kmsg, ALWAYS, NONE) */
                let _ = ipc_mqueue_send(
                    kmsg,
                    crate::mach_types::MACH_SEND_ALWAYS,
                    MACH_MSG_TIMEOUT_NONE,
                );
                return MACH_SEND_WILL_NOTIFY;
            }
        }
    } else {
        mr = ipc_mqueue_send(kmsg, option & MACH_SEND_TIMEOUT, time_out);
    }

    if mr != MACH_MSG_SUCCESS {
        mr |= ipc_kmsg_copyout_pseudo(kmsg, space, map);
        crate::kassert!((*kmsg).ikm_marequest == IMAR_NULL, "kmsg->ikm_marequest == IMAR_NULL");
        let _ = IMAR_NULL;
        let _ = ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
    }

    mr
}

// ---------------------------------------------------------------------------
//  mach_msg_receive
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_msg_receive(
    msg: *mut mach_msg_user_header_t,
    option: mach_msg_option_t,
    rcv_size: mach_msg_size_t,
    rcv_name: mach_port_name_t,
    time_out: mach_msg_timeout_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    let self_th = current_thread();
    let space = current_space();
    let map = current_map();

    let mut object: *mut ipc_object = null_mut();
    let mut mqueue: *mut ipc_mqueue = null_mut();
    let mut kmsg: *mut ipc_kmsg_full = null_mut();
    let mut seqno: mach_port_seqno_t = 0;

    let mr = ipc_mqueue_copyin(space, rcv_name, &mut mqueue, &mut object);
    if mr != MACH_MSG_SUCCESS {
        return mr;
    }
    /* hold ref for object; mqueue is locked */

    /* Save state for mach_msg_receive_continue */
    (*saved_recv(self_th)).msg = msg;
    (*saved_recv(self_th)).option = option;
    (*saved_recv(self_th)).rcv_size = rcv_size;
    (*saved_recv(self_th)).timeout = time_out;
    (*saved_recv(self_th)).notify = notify;
    (*saved_recv(self_th)).object = object;
    (*saved_recv(self_th)).mqueue = mqueue;

    let cont: crate::mach_types::continuation_t = Some(mach_msg_receive_continue_thunk);

    if (option & MACH_RCV_LARGE) != 0 {
        let mr = ipc_mqueue_receive(
            mqueue,
            option & MACH_RCV_TIMEOUT,
            rcv_size,
            time_out,
            0,
            cont,
            &mut kmsg,
            &mut seqno,
        );
        ipc_object_release(object);
        if mr != MACH_MSG_SUCCESS {
            if mr == MACH_RCV_TOO_LARGE {
                let real_size = kmsg as usize as mach_msg_size_t;
                let _ = copyout(
                    &real_size as *const mach_msg_size_t as *const _,
                    addr_of_mut!((*msg).msgh_size) as *mut _,
                    core::mem::size_of::<mach_msg_size_t>(),
                );
            }
            return mr;
        }
        (*kmsg).ikm_header.msgh_seqno = seqno;
    } else {
        let mr = ipc_mqueue_receive(
            mqueue,
            option & MACH_RCV_TIMEOUT,
            MACH_MSG_SIZE_MAX,
            time_out,
            0,
            cont,
            &mut kmsg,
            &mut seqno,
        );
        ipc_object_release(object);
        if mr != MACH_MSG_SUCCESS {
            return mr;
        }
        (*kmsg).ikm_header.msgh_seqno = seqno;
        if msg_usize(&(*kmsg).ikm_header) > rcv_size as usize {
            ipc_kmsg_copyout_dest(kmsg, space);
            let _ = ipc_kmsg_put(
                msg,
                kmsg,
                core::mem::size_of::<mach_msg_user_header_t>() as mach_msg_size_t,
            );
            return MACH_RCV_TOO_LARGE;
        }
    }

    let mr = if (option & MACH_RCV_NOTIFY) != 0 {
        if notify == MACH_PORT_NAME_NULL {
            MACH_RCV_INVALID_NOTIFY
        } else {
            ipc_kmsg_copyout(kmsg, space, map, notify)
        }
    } else {
        ipc_kmsg_copyout(kmsg, space, map, MACH_PORT_NAME_NULL)
    };
    if mr != MACH_MSG_SUCCESS {
        if (mr & !MACH_MSG_MASK) == MACH_RCV_BODY_ERROR {
            let _ = ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
        } else {
            ipc_kmsg_copyout_dest(kmsg, space);
            let _ = ipc_kmsg_put(
                msg,
                kmsg,
                core::mem::size_of::<mach_msg_user_header_t>() as mach_msg_size_t,
            );
        }
        return mr;
    }

    ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size)
}

/// Wrapper for the continuation function pointer (its declared return type
/// is `!` due to the C `void (*)(void)` typedef being modeled that way in
/// our mach_types.rs).  Calls `mach_msg_receive_continue` and never returns.
unsafe extern "C" fn mach_msg_receive_continue_thunk() -> ! {
    mach_msg_receive_continue();
}

unsafe extern "C" fn mach_msg_continue_thunk() -> ! {
    mach_msg_continue();
}

// ---------------------------------------------------------------------------
//  mach_msg_receive_continue
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_msg_receive_continue() -> ! {
    let self_th = current_thread();
    let space = current_space();
    let map = current_map();

    let msg = (*saved_recv(self_th)).msg;
    let option = (*saved_recv(self_th)).option;
    let rcv_size = (*saved_recv(self_th)).rcv_size;
    let time_out = (*saved_recv(self_th)).timeout;
    let notify = (*saved_recv(self_th)).notify;
    let object = (*saved_recv(self_th)).object;
    let mqueue = (*saved_recv(self_th)).mqueue;

    let mut kmsg: *mut ipc_kmsg_full = null_mut();
    let mut seqno: mach_port_seqno_t = 0;
    let cont: crate::mach_types::continuation_t = Some(mach_msg_receive_continue_thunk);

    if (option & MACH_RCV_LARGE) != 0 {
        let mr = ipc_mqueue_receive(
            mqueue,
            option & MACH_RCV_TIMEOUT,
            rcv_size,
            time_out,
            1,
            cont,
            &mut kmsg,
            &mut seqno,
        );
        ipc_object_release(object);
        if mr != MACH_MSG_SUCCESS {
            if mr == MACH_RCV_TOO_LARGE {
                let real_size = kmsg as usize as mach_msg_size_t;
                let _ = copyout(
                    &real_size as *const mach_msg_size_t as *const _,
                    addr_of_mut!((*msg).msgh_size) as *mut _,
                    core::mem::size_of::<mach_msg_size_t>(),
                );
            }
            thread_syscall_return(mr);
        }
        (*kmsg).ikm_header.msgh_seqno = seqno;
    } else {
        let mr = ipc_mqueue_receive(
            mqueue,
            option & MACH_RCV_TIMEOUT,
            MACH_MSG_SIZE_MAX,
            time_out,
            1,
            cont,
            &mut kmsg,
            &mut seqno,
        );
        ipc_object_release(object);
        if mr != MACH_MSG_SUCCESS {
            thread_syscall_return(mr);
        }
        (*kmsg).ikm_header.msgh_seqno = seqno;
        if msg_usize(&(*kmsg).ikm_header) > rcv_size as usize {
            ipc_kmsg_copyout_dest(kmsg, space);
            let _ = ipc_kmsg_put(
                msg,
                kmsg,
                core::mem::size_of::<mach_msg_user_header_t>() as mach_msg_size_t,
            );
            thread_syscall_return(MACH_RCV_TOO_LARGE);
        }
    }

    let mr = if (option & MACH_RCV_NOTIFY) != 0 {
        if notify == MACH_PORT_NAME_NULL {
            MACH_RCV_INVALID_NOTIFY
        } else {
            ipc_kmsg_copyout(kmsg, space, map, notify)
        }
    } else {
        ipc_kmsg_copyout(kmsg, space, map, MACH_PORT_NAME_NULL)
    };
    if mr != MACH_MSG_SUCCESS {
        if (mr & !MACH_MSG_MASK) == MACH_RCV_BODY_ERROR {
            let _ = ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
        } else {
            ipc_kmsg_copyout_dest(kmsg, space);
            let _ = ipc_kmsg_put(
                msg,
                kmsg,
                core::mem::size_of::<mach_msg_user_header_t>() as mach_msg_size_t,
            );
        }
        thread_syscall_return(mr);
    }

    let mr = ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
    thread_syscall_return(mr);
}

// ---------------------------------------------------------------------------
//  mach_msg_continue
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_msg_continue() -> ! {
    let thread = current_thread();
    let task = (*thread).task;
    let space = task_itk_space(task);
    let map = task_map(task);

    let msg = (*saved_recv(thread)).msg;
    let rcv_size = (*saved_recv(thread)).rcv_size;
    let object = (*saved_recv(thread)).object;
    let mqueue = (*saved_recv(thread)).mqueue;

    let mut kmsg: *mut ipc_kmsg_full = null_mut();
    let mut seqno: mach_port_seqno_t = 0;
    let cont: crate::mach_types::continuation_t = Some(mach_msg_continue_thunk);

    let mr = ipc_mqueue_receive(
        mqueue,
        MACH_MSG_OPTION_NONE,
        MACH_MSG_SIZE_MAX,
        MACH_MSG_TIMEOUT_NONE,
        1,
        cont,
        &mut kmsg,
        &mut seqno,
    );
    ipc_object_release(object);
    if mr != MACH_MSG_SUCCESS {
        thread_syscall_return(mr);
    }

    (*kmsg).ikm_header.msgh_seqno = seqno;
    if msg_usize(&(*kmsg).ikm_header) > rcv_size as usize {
        ipc_kmsg_copyout_dest(kmsg, space);
        let _ = ipc_kmsg_put(
            msg,
            kmsg,
            core::mem::size_of::<mach_msg_user_header_t>() as mach_msg_size_t,
        );
        thread_syscall_return(MACH_RCV_TOO_LARGE);
    }

    let mr = ipc_kmsg_copyout(kmsg, space, map, MACH_PORT_NAME_NULL);
    if mr != MACH_MSG_SUCCESS {
        if (mr & !MACH_MSG_MASK) == MACH_RCV_BODY_ERROR {
            let _ = ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
        } else {
            ipc_kmsg_copyout_dest(kmsg, space);
            let _ = ipc_kmsg_put(
                msg,
                kmsg,
                core::mem::size_of::<mach_msg_user_header_t>() as mach_msg_size_t,
            );
        }
        thread_syscall_return(mr);
    }

    let mr = ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
    thread_syscall_return(mr);
}

// ---------------------------------------------------------------------------
//  mach_msg_trap_combined — optimised combined-option RPC path.
// ---------------------------------------------------------------------------
//
// Mirrors the `option == (MACH_SEND_MSG | MACH_RCV_MSG)` arm of the original
// C `mach_msg_trap`.  Step 1b deliverable: `fast_get` + `fast_copyin` request
// case + `fast_send_receive` branch (a) thread-handoff to `mach_msg_continue`.
// All other fast-path optimisations (reply-case copyin, exception/receive
// handoff branches, fast_copyout, fast_put, kernel_send) fall through to the
// equivalent slow-path call into already-ported Rust IPC primitives.

/// State labels matching the `goto`-driven C code at `ipc/mach_msg.c:391+`.
enum CombinedState {
    FastGet,
    FastCopyin,
    FastSendReceive,
    FastCopyout,
    FastPut,
    KernelSend,
    SlowCopyin,
    SlowSend,
    SlowGetRcvPort,
    SlowReceive,
    SlowCopyout,
    SlowPut,
}

unsafe fn mach_msg_trap_combined(
    msg_in: *mut mach_msg_user_header_t,
    send_size: mach_msg_size_t,
    rcv_size_in: mach_msg_size_t,
    rcv_name: mach_port_name_t,
) -> ! {
    use CombinedState::*;

    /* Mutable thread-context locals — these track the "current" thread,
     * which can change after thread_handoff in fast_send_receive. */
    let mut self_th: ipc_thread_t = current_thread();
    let mut space: ipc_space_t = task_itk_space((*self_th).task);
    let mut msg = msg_in;
    let mut rcv_size = rcv_size_in;

    let mut kmsg: *mut ipc_kmsg_full = null_mut();
    let mut dest_port: ipc_port_t = null_mut();
    let mut rcv_object: *mut ipc_object = null_mut();
    let mut rcv_mqueue: *mut ipc_mqueue = null_mut();
    let mut reply_size: mach_msg_size_t = 0;

    /* When fast_get fails, we re-enter via slow_get → fast_copyin.  Track
     * that with a flag. */
    let mut took_slow_get: bool = false;

    let mut state = FastGet;

    loop {
        match state {
            // --------------------------------------------------------------
            //  fast_get / slow_get
            // --------------------------------------------------------------
            FastGet => {
                let mut want_slow = took_slow_get;
                if !want_slow {
                    'fast_get: {
                        if (send_size * IKM_EXPAND_FACTOR) > IKM_SAVED_MSG_SIZE
                            || (send_size as usize)
                                < core::mem::size_of::<mach_msg_user_header_t>()
                            || (send_size & 3) != 0
                        {
                            want_slow = true;
                            break 'fast_get;
                        }
                        kmsg = ikm_cache_alloc_try();
                        if kmsg.is_null() {
                            want_slow = true;
                            break 'fast_get;
                        }
                        if copyinmsg(
                            msg as *const core::ffi::c_void,
                            addr_of_mut!((*kmsg).ikm_header)
                                as *mut core::ffi::c_void,
                            send_size as usize,
                            (*kmsg).ikm_size as usize,
                        ) != 0
                        {
                            ikm_free(kmsg);
                            want_slow = true;
                            break 'fast_get;
                        }
                        /* fast_get success — fall through to fast_copyin */
                    }
                }
                if want_slow {
                    /* slow_get */
                    let mut tk: *mut ipc_kmsg_full = null_mut();
                    let mr = ipc_kmsg_get(msg, send_size, &mut tk);
                    if mr != MACH_MSG_SUCCESS {
                        thread_syscall_return(mr);
                    }
                    kmsg = tk;
                    took_slow_get = true;
                    /* fall to fast_copyin */
                }
                state = FastCopyin;
            }

            // --------------------------------------------------------------
            //  fast_copyin — request case only (Step 1b); reply / default
            //  fall through to slow_copyin.
            // --------------------------------------------------------------
            FastCopyin => {
                let bits = (*kmsg).ikm_header.msgh_bits;
                let request_pat = crate::mach_types::MACH_MSGH_BITS(
                    MACH_MSG_TYPE_COPY_SEND,
                    MACH_MSG_TYPE_MAKE_SEND_ONCE,
                );
                let reply_pat = crate::mach_types::MACH_MSGH_BITS(
                    MACH_MSG_TYPE_MOVE_SEND_ONCE,
                    0,
                );

                if bits == reply_pat {
                    /* === reply-message case === */
                    let mut to_slow_send = false;
                    let mut to_fast_send_receive = false;

                    'reply: {
                        if (*kmsg).ikm_header.msgh_local_port != MACH_PORT_NULL
                        {
                            break 'reply;
                        }

                        is_write_lock(space);
                        crate::kassert!(
                            (*space).is_active != 0,
                            "space->is_active"
                        );

                        let dest_name = (*kmsg).ikm_header.msgh_remote_port;
                        let entry: ipc_entry_t =
                            ipc_entry_lookup(space, dest_name);
                        if entry.is_null() {
                            ipc_entry_lookup_failed(msg, dest_name);
                            is_write_unlock(space);
                            break 'reply;
                        }
                        let entry_bits = (*entry).ie_bits;
                        if ie_bits_type(entry_bits)
                            != MACH_PORT_TYPE_SEND_ONCE
                        {
                            is_write_unlock(space);
                            break 'reply;
                        }

                        crate::kassert!(
                            (entry_bits & IE_BITS_UREFS_MASK) == 1,
                            "IE_BITS_UREFS(bits) == 1"
                        );
                        crate::kassert!(
                            (entry_bits & IE_BITS_MAREQUEST) == 0,
                            "(bits & IE_BITS_MAREQUEST) == 0"
                        );

                        if (*entry).index.request != 0 {
                            is_write_unlock(space);
                            break 'reply;
                        }

                        dest_port = (*entry).ie_object as ipc_port_t;
                        crate::kassert!(
                            !dest_port.is_null(),
                            "dest_port != IP_NULL"
                        );

                        ip_lock(dest_port);
                        if !ip_active(dest_port) {
                            ip_unlock(dest_port);
                            is_write_unlock(space);
                            break 'reply;
                        }

                        crate::kassert!(
                            (*dest_port).ip_sorights > 0,
                            "dest_port->ip_sorights > 0"
                        );
                        (*entry).ie_object = IO_NULL;
                        ipc_entry_dealloc(space, dest_name, entry);

                        (*kmsg).ikm_header.msgh_bits =
                            crate::mach_types::MACH_MSGH_BITS(
                                MACH_MSG_TYPE_PORT_SEND_ONCE,
                                0,
                            );
                        (*kmsg).ikm_header.msgh_remote_port =
                            dest_port as usize as u32;

                        crate::kassert!(
                            (*dest_port).data.receiver
                                != crate::ipc_space::ipc_space_kernel,
                            "dest_port->ip_receiver != ipc_space_kernel"
                        );

                        /* optimised ipc_mqueue_copyin */
                        let rcv_entry: ipc_entry_t =
                            ipc_entry_lookup(space, rcv_name);
                        if rcv_entry.is_null() {
                            ipc_entry_lookup_failed(msg, rcv_name);
                            /* abort_reply_rcv_copyin */
                            ip_unlock(dest_port);
                            is_write_unlock(space);
                            to_slow_send = true;
                            break 'reply;
                        }
                        let rcv_bits = (*rcv_entry).ie_bits;

                        if (rcv_bits & MACH_PORT_TYPE_PORT_SET) != 0 {
                            let rcv_pset: ipc_pset_t =
                                (*rcv_entry).ie_object as ipc_pset_t;
                            crate::kassert!(
                                !rcv_pset.is_null(),
                                "rcv_pset != IPS_NULL"
                            );
                            ips_lock(rcv_pset);
                            crate::kassert!(
                                ips_active(rcv_pset),
                                "ips_active(rcv_pset)"
                            );
                            rcv_object = rcv_pset as *mut ipc_object;
                            rcv_mqueue = addr_of_mut!(
                                (*rcv_pset).ips_target.ipt_messages
                            );
                        } else if (rcv_bits & MACH_PORT_TYPE_RECEIVE) != 0 {
                            let rcv_port: ipc_port_t =
                                (*rcv_entry).ie_object as ipc_port_t;
                            crate::kassert!(
                                !rcv_port.is_null(),
                                "rcv_port != IP_NULL"
                            );

                            if !ip_lock_try(rcv_port) {
                                /* abort_reply_rcv_copyin */
                                ip_unlock(dest_port);
                                is_write_unlock(space);
                                to_slow_send = true;
                                break 'reply;
                            }
                            crate::kassert!(
                                ip_active(rcv_port),
                                "ip_active(rcv_port)"
                            );
                            if (*rcv_port).ip_pset != IPS_NULL {
                                ip_unlock(rcv_port);
                                /* abort_reply_rcv_copyin */
                                ip_unlock(dest_port);
                                is_write_unlock(space);
                                to_slow_send = true;
                                break 'reply;
                            }
                            rcv_object = rcv_port as *mut ipc_object;
                            rcv_mqueue = addr_of_mut!(
                                (*rcv_port).ip_target.ipt_messages
                            );
                        } else {
                            /* abort_reply_rcv_copyin */
                            ip_unlock(dest_port);
                            is_write_unlock(space);
                            to_slow_send = true;
                            break 'reply;
                        }

                        is_write_unlock(space);
                        (*rcv_object).io_references += 1;
                        imq_lock(rcv_mqueue);
                        io_unlock(rcv_object);
                        to_fast_send_receive = true;
                    }

                    if to_fast_send_receive {
                        state = FastSendReceive;
                    } else if to_slow_send {
                        state = SlowSend;
                    } else {
                        state = SlowCopyin;
                    }
                    continue;
                }

                if bits != request_pat {
                    /* default: any other bits → slow_copyin */
                    state = SlowCopyin;
                    continue;
                }

                /* === request-message case === */
                let mut to_kernel_send = false;
                let mut to_slow_send = false;
                let mut to_fast_send_receive = false;

                'request: {
                    let reply_name = (*kmsg).ikm_header.msgh_local_port;
                    if reply_name != rcv_name {
                        break 'request;
                    }

                    is_read_lock(space);
                    crate::kassert!((*space).is_active != 0, "space->is_active");

                    let r_entry: ipc_entry_t = ipc_entry_lookup(space, reply_name);
                    if r_entry.is_null() {
                        ipc_entry_lookup_failed(msg, reply_name);
                        is_read_unlock(space);
                        break 'request;
                    }
                    let reply_port: ipc_port_t =
                        (*r_entry).ie_object as ipc_port_t;
                    crate::kassert!(!reply_port.is_null(), "reply_port != IP_NULL");

                    let dest_name = (*kmsg).ikm_header.msgh_remote_port;
                    let d_entry: ipc_entry_t = ipc_entry_lookup(space, dest_name);
                    if d_entry.is_null() {
                        ipc_entry_lookup_failed(msg, dest_name);
                        is_read_unlock(space);
                        break 'request;
                    }
                    let d_bits = (*d_entry).ie_bits;
                    if (d_bits & IE_BITS_TYPE_MASK) != MACH_PORT_TYPE_SEND {
                        is_read_unlock(space);
                        break 'request;
                    }
                    crate::kassert!(
                        (d_bits & IE_BITS_UREFS_MASK) > 0,
                        "IE_BITS_UREFS(bits) > 0"
                    );
                    dest_port = (*d_entry).ie_object as ipc_port_t;
                    crate::kassert!(!dest_port.is_null(), "dest_port != IP_NULL");

                    /* simultaneous lock attempt — NCPUS=1 always succeeds */
                    ip_lock(dest_port);
                    if !ip_active(dest_port) {
                        ip_unlock(dest_port);
                        is_read_unlock(space);
                        break 'request;
                    }
                    is_read_unlock(space);

                    crate::kassert!(
                        (*dest_port).ip_srights > 0,
                        "dest_port->ip_srights > 0"
                    );
                    (*dest_port).ip_srights += 1;
                    ipc_port_reference(dest_port);

                    crate::kassert!(ip_active(reply_port), "ip_active(reply_port)");
                    crate::kassert!(
                        (*reply_port).ip_target.ipt_name
                            == (*kmsg).ikm_header.msgh_local_port,
                        "reply_port->ip_receiver_name == msgh_local_port"
                    );
                    crate::kassert!(
                        (*reply_port).data.receiver == space,
                        "reply_port->ip_receiver == space"
                    );

                    (*reply_port).ip_sorights += 1;
                    ipc_port_reference(reply_port);

                    (*kmsg).ikm_header.msgh_bits =
                        crate::mach_types::MACH_MSGH_BITS(
                            MACH_MSG_TYPE_PORT_SEND,
                            MACH_MSG_TYPE_PORT_SEND_ONCE,
                        );
                    (*kmsg).ikm_header.msgh_remote_port =
                        dest_port as usize as u32;
                    (*kmsg).ikm_header.msgh_local_port =
                        reply_port as usize as u32;

                    /* Late-fail conditions: kernel-dest, qlimit, reply pset */
                    if (*dest_port).data.receiver
                        == crate::ipc_space::ipc_space_kernel
                    {
                        /* The kernel server holds a reference to the reply
                         * port and hands it back in the reply message; we
                         * don't need the extra reference here. */
                        ip_unlock(reply_port);
                        crate::kassert!(
                            ip_active(dest_port),
                            "ip_active(dest_port)"
                        );
                        ip_unlock(dest_port);
                        to_kernel_send = true;
                        break 'request;
                    }
                    if (*dest_port).ip_msgcount >= (*dest_port).ip_qlimit {
                        ip_unlock(dest_port);
                        ip_unlock(reply_port);
                        to_slow_send = true;
                        break 'request;
                    }
                    if (*reply_port).ip_pset != IPS_NULL {
                        ip_unlock(dest_port);
                        ip_unlock(reply_port);
                        to_slow_send = true;
                        break 'request;
                    }

                    /* optimised ipc_mqueue_copyin */
                    rcv_object = reply_port as *mut ipc_object;
                    /* io_reference(rcv_object) */
                    (*rcv_object).io_references += 1;
                    rcv_mqueue =
                        addr_of_mut!((*reply_port).ip_target.ipt_messages);
                    imq_lock(rcv_mqueue);
                    io_unlock(rcv_object);
                    to_fast_send_receive = true;
                }

                if to_fast_send_receive {
                    state = FastSendReceive;
                } else if to_kernel_send {
                    state = KernelSend;
                } else if to_slow_send {
                    state = SlowSend;
                } else {
                    /* to_slow_copyin */
                    state = SlowCopyin;
                }
            }

            // --------------------------------------------------------------
            //  fast_send_receive — branch (a) only (Step 1b).
            // --------------------------------------------------------------
            FastSendReceive => {
                crate::kassert!(ip_active(dest_port), "ip_active(dest_port)");
                crate::kassert!(
                    (*dest_port).data.receiver
                        != crate::ipc_space::ipc_space_kernel,
                    "dest_port->ip_receiver != ipc_space_kernel"
                );

                let dest_pset: ipc_pset_t = (*dest_port).ip_pset;
                let dest_mqueue: *mut ipc_mqueue = if dest_pset == IPS_NULL {
                    addr_of_mut!((*dest_port).ip_target.ipt_messages)
                } else {
                    addr_of_mut!((*dest_pset).ips_target.ipt_messages)
                };

                if !imq_lock_try(dest_mqueue) {
                    /* abort_send_receive */
                    ip_unlock(dest_port);
                    imq_unlock(rcv_mqueue);
                    ipc_object_release(rcv_object);
                    state = SlowSend;
                    continue;
                }

                let receiver: ipc_thread_t = ipc_thread_queue_first(
                    addr_of_mut!((*dest_mqueue).imq_threads),
                );
                if receiver == ITH_NULL
                    || !ipc_kmsg_queue_first(addr_of_mut!(
                        (*rcv_mqueue).imq_messages
                    ))
                    .is_null()
                {
                    imq_unlock(dest_mqueue);
                    /* abort_send_receive */
                    ip_unlock(dest_port);
                    imq_unlock(rcv_mqueue);
                    ipc_object_release(rcv_object);
                    state = SlowSend;
                    continue;
                }

                /* Save self's state for its eventual receive of the reply */
                (*saved_recv(self_th)).msg = msg;
                (*saved_recv(self_th)).rcv_size = rcv_size;
                (*saved_recv(self_th)).object = rcv_object;
                (*saved_recv(self_th)).mqueue = rcv_mqueue;

                /* Three handoff variants — see ipc/mach_msg.c:805+
                 * (a) receiver running mach_msg_continue       → common epilogue
                 * (b) receiver running exception_raise_continue → exception fast path
                 * (c) send_size <= receiver->ith_msize          → swap_func / common epilogue
                 *
                 * All variants pass `mach_msg_continue` as *self*'s continuation
                 * so that when self resumes (after the reply arrives) it picks
                 * up via mach_msg_continue. */
                let mach_msg_cont_fn: unsafe extern "C" fn() -> ! =
                    mach_msg_continue_thunk;
                let mach_msg_cont_addr: usize = mach_msg_cont_fn as usize;
                let mach_msg_recv_cont_fn: unsafe extern "C" fn() -> ! =
                    mach_msg_receive_continue_thunk;
                let mach_msg_recv_continue_addr: usize =
                    mach_msg_recv_cont_fn as usize;
                let exception_raise_continue_fn: unsafe extern "C" fn() -> ! =
                    exception_raise_continue;
                let exception_raise_continue_addr: usize =
                    exception_raise_continue_fn as usize;

                let receiver_swap_addr: usize = match (*receiver).swap_func {
                    Some(f) => f as usize,
                    None => 0,
                };

                let mut handed_off_a = false;
                let mut handed_off_b = false;
                let mut handed_off_c = false;

                if receiver_swap_addr == mach_msg_cont_addr
                    && thread_handoff(
                        self_th,
                        Some(mach_msg_continue_thunk),
                        receiver,
                    ) != 0
                {
                    handed_off_a = true;
                } else if receiver_swap_addr == exception_raise_continue_addr
                    && thread_handoff(
                        self_th,
                        Some(mach_msg_continue_thunk),
                        receiver,
                    ) != 0
                {
                    handed_off_b = true;
                } else if (send_size as u32) <= (*receiver).data.msize
                    && thread_handoff(
                        self_th,
                        Some(mach_msg_continue_thunk),
                        receiver,
                    ) != 0
                {
                    handed_off_c = true;
                }

                if !(handed_off_a || handed_off_b || handed_off_c) {
                    /* Can't switch to the receiver — abort_send_receive */
                    imq_unlock(dest_mqueue);
                    ip_unlock(dest_port);
                    imq_unlock(rcv_mqueue);
                    ipc_object_release(rcv_object);
                    state = SlowSend;
                    continue;
                }

                crate::kassert!(
                    current_thread() == receiver,
                    "current_thread() == receiver"
                );

                if handed_off_b {
                    /* Branch (b): the receiver is in the optimized exception
                     * handler.  Finish queue plumbing and tail-call
                     * exception_raise_continue_fast with dest_port still
                     * locked.  No sequence number for this case. */
                    ipc_thread_enqueue_macro(
                        addr_of_mut!((*rcv_mqueue).imq_threads),
                        self_th,
                    );
                    (*self_th).ith_state = MACH_RCV_IN_PROGRESS;
                    (*self_th).data.msize = MACH_MSG_SIZE_MAX;
                    imq_unlock(rcv_mqueue);

                    ipc_thread_rmqueue_first_macro(
                        addr_of_mut!((*dest_mqueue).imq_threads),
                        receiver,
                    );
                    imq_unlock(dest_mqueue);

                    exception_raise_continue_fast(dest_port, kmsg);
                    /* NOTREACHED */
                }

                if handed_off_c {
                    /* Branch (c) sub-cases.  After the size-fits handoff we
                     * either continue with the optimized common epilogue
                     * (sub-case 1) or deliver the kmsg to the receiver via
                     * its swap_func (sub-case 2). */
                    let receiver_swap_addr_now: usize =
                        match (*receiver).swap_func {
                            Some(f) => f as usize,
                            None => 0,
                        };
                    let r_option =
                        (*saved_recv(receiver)).option;

                    if receiver_swap_addr_now == mach_msg_recv_continue_addr
                        && (r_option & MACH_RCV_NOTIFY) == 0
                    {
                        /* Sub-case 1: optimized — fall through to common
                         * epilogue below. */
                    } else {
                        /* Sub-case 2: deliver the kmsg via the receiver's
                         * own continuation. */
                        (*dest_port).ip_msgcount += 1;
                        ip_unlock(dest_port);

                        ipc_thread_enqueue_macro(
                            addr_of_mut!((*rcv_mqueue).imq_threads),
                            self_th,
                        );
                        (*self_th).ith_state = MACH_RCV_IN_PROGRESS;
                        (*self_th).data.msize = MACH_MSG_SIZE_MAX;
                        imq_unlock(rcv_mqueue);

                        ipc_thread_rmqueue_first_macro(
                            addr_of_mut!((*dest_mqueue).imq_threads),
                            receiver,
                        );
                        (*receiver).ith_state = MACH_MSG_SUCCESS;
                        (*receiver).data.kmsg = kmsg as *mut ipc_kmsg;
                        (*receiver).ith_seqno = (*dest_port).ip_seqno;
                        (*dest_port).ip_seqno += 1;
                        imq_unlock(dest_mqueue);

                        (*receiver).wait_result = THREAD_AWAKENED;
                        let cont_fn = (*receiver).swap_func.expect(
                            "receiver->swap_func != NULL after handoff",
                        );
                        cont_fn();
                        /* NOTREACHED */
                    }
                }

                /* Common epilogue: branch (a) or branch (c) sub-case 1
                 * (line 893+ in the C). */
                ip_unlock(dest_port);
                ipc_thread_enqueue_macro(
                    addr_of_mut!((*rcv_mqueue).imq_threads),
                    self_th,
                );
                (*self_th).ith_state = MACH_RCV_IN_PROGRESS;
                (*self_th).data.msize = MACH_MSG_SIZE_MAX;
                imq_unlock(rcv_mqueue);

                ipc_thread_rmqueue_first_macro(
                    addr_of_mut!((*dest_mqueue).imq_threads),
                    receiver,
                );
                (*kmsg).ikm_header.msgh_seqno = (*dest_port).ip_seqno;
                (*dest_port).ip_seqno += 1;
                imq_unlock(dest_mqueue);

                /* Now we ARE the receiver. */
                self_th = receiver;
                space = task_itk_space((*self_th).task);
                msg = (*saved_recv(self_th)).msg;
                rcv_size = (*saved_recv(self_th)).rcv_size;
                rcv_object = (*saved_recv(self_th)).object;

                /* inline ipc_object_release */
                io_lock(rcv_object);
                io_release(rcv_object);
                io_check_unlock(rcv_object);

                state = FastCopyout;
            }

            // --------------------------------------------------------------
            //  fast_copyout — request case (Step 1c).  Reply case + default
            //  fall through to slow_copyout (Step 1d).
            // --------------------------------------------------------------
            FastCopyout => {
                crate::kassert!(
                    (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t
                        == dest_port,
                    "kmsg->msgh_remote_port == dest_port"
                );

                reply_size = (*kmsg).ikm_header.msgh_size;
                if (rcv_size as usize) < msg_usize(&(*kmsg).ikm_header) {
                    state = SlowCopyout;
                    continue;
                }

                let bits = (*kmsg).ikm_header.msgh_bits;
                let req_pat = crate::mach_types::MACH_MSGH_BITS(
                    MACH_MSG_TYPE_PORT_SEND,
                    MACH_MSG_TYPE_PORT_SEND_ONCE,
                );
                let reply_pat = crate::mach_types::MACH_MSGH_BITS(
                    MACH_MSG_TYPE_PORT_SEND_ONCE,
                    0,
                );

                if bits == reply_pat {
                    /* === reply-message case (receiving an RPC reply) === */
                    let mut dest_name: mach_port_name_t;
                    let payload: u32;

                    ip_lock(dest_port);
                    if !ip_active(dest_port) {
                        ip_unlock(dest_port);
                        state = SlowCopyout;
                        continue;
                    }

                    crate::kassert!(
                        (*dest_port).ip_sorights > 0,
                        "dest_port->ip_sorights > 0"
                    );

                    payload = (*dest_port).ip_protected_payload;

                    if (*dest_port).data.receiver == space {
                        ip_release(dest_port);
                        (*dest_port).ip_sorights -= 1;
                        dest_name = (*dest_port).ip_target.ipt_name;
                        ip_unlock(dest_port);
                    } else {
                        ip_unlock(dest_port);
                        ipc_notify_send_once(dest_port);
                        dest_name = MACH_PORT_NULL;
                    }

                    if !ipc_port_flag_protected_payload(dest_port) {
                        (*kmsg).ikm_header.msgh_bits =
                            crate::mach_types::MACH_MSGH_BITS(
                                0,
                                MACH_MSG_TYPE_PORT_SEND_ONCE,
                            );
                        (*kmsg).ikm_header.msgh_local_port = dest_name;
                    } else {
                        (*kmsg).ikm_header.msgh_bits =
                            crate::mach_types::MACH_MSGH_BITS(
                                0,
                                MACH_MSG_TYPE_PROTECTED_PAYLOAD,
                            );
                        (*kmsg).ikm_header.msgh_local_port = payload;
                    }
                    (*kmsg).ikm_header.msgh_remote_port = MACH_PORT_NULL;
                    state = FastPut;
                    continue;
                }

                if bits != req_pat {
                    /* default: fall to slow_copyout */
                    state = SlowCopyout;
                    continue;
                }

                /* === request-message case (receiving an RPC request) === */
                let reply_port: ipc_port_t =
                    (*kmsg).ikm_header.msgh_local_port as usize as ipc_port_t;
                if reply_port.is_null() || (reply_port as usize) == !0usize {
                    /* !IP_VALID(reply_port) */
                    state = SlowCopyout;
                    continue;
                }

                let mut to_slow_copyout = false;
                let mut reply_name: mach_port_name_t = 0;
                let mut dest_name: mach_port_name_t = 0;
                let mut payload: u32 = 0;

                'request_copyout: {
                    is_write_lock(space);
                    crate::kassert!((*space).is_active != 0, "space->is_active");

                    ip_lock(dest_port);
                    if !ip_active(dest_port) || !ip_lock_try(reply_port) {
                        /* abort_request_copyout */
                        ip_unlock(dest_port);
                        is_write_unlock(space);
                        to_slow_copyout = true;
                        break 'request_copyout;
                    }

                    if !ip_active(reply_port) {
                        ip_unlock(reply_port);
                        ip_unlock(dest_port);
                        is_write_unlock(space);
                        to_slow_copyout = true;
                        break 'request_copyout;
                    }

                    crate::kassert!(
                        (*reply_port).ip_sorights > 0,
                        "reply_port->ip_sorights > 0"
                    );
                    ip_unlock(reply_port);

                    let mut entry: ipc_entry_t = core::ptr::null_mut();
                    let kr = ipc_entry_get(space, &mut reply_name, &mut entry);
                    if kr != KERN_SUCCESS {
                        /* abort_request_copyout */
                        ip_unlock(dest_port);
                        is_write_unlock(space);
                        to_slow_copyout = true;
                        break 'request_copyout;
                    }
                    crate::kassert!(!entry.is_null(), "entry != NULL");

                    /* IE_BITS_GEN_MASK == 0 / IE_BITS_GEN_ONE == 0 on i686 */
                    let gen: u32 = (*entry).ie_bits;
                    /* optimised ipc_right_copyout */
                    (*entry).ie_bits = gen | (MACH_PORT_TYPE_SEND_ONCE | 1);

                    crate::kassert!(
                        mach_port_name_valid(reply_name),
                        "MACH_PORT_NAME_VALID(reply_name)"
                    );
                    (*entry).ie_object = reply_port as *mut ipc_object;
                    is_write_unlock(space);

                    /* optimised ipc_object_copyout_dest */
                    crate::kassert!(
                        (*dest_port).ip_srights > 0,
                        "dest_port->ip_srights > 0"
                    );
                    ip_release(dest_port);

                    if (*dest_port).data.receiver == space {
                        dest_name = (*dest_port).ip_target.ipt_name;
                    } else {
                        dest_name = MACH_PORT_NULL;
                    }
                    payload = (*dest_port).ip_protected_payload;

                    (*dest_port).ip_srights -= 1;
                    if (*dest_port).ip_srights == 0
                        && !(*dest_port).ip_nsrequest.is_null()
                    {
                        let nsrequest = (*dest_port).ip_nsrequest;
                        let mscount = (*dest_port).ip_mscount;
                        (*dest_port).ip_nsrequest = core::ptr::null_mut();
                        ip_unlock(dest_port);
                        ipc_notify_no_senders(nsrequest, mscount);
                    } else {
                        ip_unlock(dest_port);
                    }
                }

                if to_slow_copyout {
                    state = SlowCopyout;
                    continue;
                }

                if !ipc_port_flag_protected_payload(dest_port) {
                    (*kmsg).ikm_header.msgh_bits =
                        crate::mach_types::MACH_MSGH_BITS(
                            MACH_MSG_TYPE_PORT_SEND_ONCE,
                            MACH_MSG_TYPE_PORT_SEND,
                        );
                    (*kmsg).ikm_header.msgh_local_port = dest_name;
                } else {
                    (*kmsg).ikm_header.msgh_bits =
                        crate::mach_types::MACH_MSGH_BITS(
                            MACH_MSG_TYPE_PORT_SEND_ONCE,
                            MACH_MSG_TYPE_PROTECTED_PAYLOAD,
                        );
                    (*kmsg).ikm_header.msgh_local_port = payload;
                }
                (*kmsg).ikm_header.msgh_remote_port = reply_name;
                state = FastPut;
            }

            // --------------------------------------------------------------
            //  fast_put — direct copyoutmsg + per-CPU cache return.
            // --------------------------------------------------------------
            FastPut => {
                ikm_check_initialized(kmsg, (*kmsg).ikm_size);

                if (*kmsg).ikm_size != IKM_SAVED_KMSG_SIZE
                    || copyoutmsg(
                        addr_of_mut!((*kmsg).ikm_header)
                            as *const core::ffi::c_void,
                        msg as *mut core::ffi::c_void,
                        reply_size as usize,
                    ) != 0
                {
                    state = SlowPut;
                    continue;
                }

                if !ikm_cache_free_try(kmsg) {
                    state = SlowPut;
                    continue;
                }

                thread_syscall_return(MACH_MSG_SUCCESS);
            }

            // --------------------------------------------------------------
            //  kernel_send — direct kobject server call with optimised
            //  reply-receive when the reply port is local.
            // --------------------------------------------------------------
            KernelSend => {
                /* The request message has been copied into kmsg.  Nothing
                 * is locked. */
                kmsg = ipc_kobject_server(kmsg);
                if kmsg.is_null() {
                    /* No reply.  Take the slow receive path. */
                    state = SlowGetRcvPort;
                    continue;
                }

                let reply_port: ipc_port_t = (*kmsg).ikm_header.msgh_remote_port
                    as usize as ipc_port_t;
                ip_lock(reply_port);

                if !ip_active(reply_port)
                    || (*reply_port).data.receiver != space
                    || (*reply_port).ip_target.ipt_name != rcv_name
                    || (*reply_port).ip_pset != IPS_NULL
                {
                    ip_unlock(reply_port);
                    let _ = ipc_mqueue_send(
                        kmsg,
                        MACH_SEND_ALWAYS,
                        MACH_MSG_TIMEOUT_NONE,
                    );
                    state = SlowGetRcvPort;
                    continue;
                }

                rcv_mqueue =
                    addr_of_mut!((*reply_port).ip_target.ipt_messages);
                imq_lock(rcv_mqueue);
                /* keep port locked, don't change ref count yet */

                if !ipc_thread_queue_first(addr_of_mut!(
                    (*rcv_mqueue).imq_threads
                ))
                .is_null()
                    || !ipc_kmsg_queue_first(addr_of_mut!(
                        (*rcv_mqueue).imq_messages
                    ))
                    .is_null()
                {
                    imq_unlock(rcv_mqueue);
                    ip_unlock(reply_port);
                    let _ = ipc_mqueue_send(
                        kmsg,
                        MACH_SEND_ALWAYS,
                        MACH_MSG_TIMEOUT_NONE,
                    );
                    state = SlowGetRcvPort;
                    continue;
                }

                crate::kassert!(
                    (*kmsg).ikm_marequest == IMAR_NULL,
                    "kmsg->ikm_marequest == IMAR_NULL"
                );
                crate::kassert!(
                    (*reply_port).ip_blocked.ithq_base == ITH_NULL,
                    "reply_port->ip_blocked is empty"
                );

                dest_port = reply_port;
                (*kmsg).ikm_header.msgh_seqno = (*dest_port).ip_seqno;
                (*dest_port).ip_seqno += 1;
                imq_unlock(rcv_mqueue);

                /* inline ipc_object_release with port still locked.
                 * Reference count was not incremented, so just check_unlock. */
                ip_check_unlock(reply_port);

                state = FastCopyout;
            }

            // --------------------------------------------------------------
            //  slow_copyin
            // --------------------------------------------------------------
            SlowCopyin => {
                let mr =
                    ipc_kmsg_copyin(kmsg, space, current_map(), MACH_PORT_NAME_NULL);
                if mr != MACH_MSG_SUCCESS {
                    ikm_free(kmsg);
                    thread_syscall_return(mr);
                }

                /* Try to detect kernel-dest and shortcut to KernelSend
                 * (mirrors the C check at slow_copyin: line 1252). */
                if (*kmsg).ikm_header.msgh_bits
                    & crate::mach_types::MACH_MSGH_BITS_CIRCULAR
                    == 0
                {
                    let dp: ipc_port_t =
                        (*kmsg).ikm_header.msgh_remote_port as usize
                            as ipc_port_t;
                    if !dp.is_null() && (dp as usize) != !0usize {
                        ip_lock(dp);
                        if (*dp).data.receiver
                            == crate::ipc_space::ipc_space_kernel
                        {
                            crate::kassert!(
                                ip_active(dp),
                                "ip_active(dest_port)"
                            );
                            ip_unlock(dp);
                            state = KernelSend;
                            continue;
                        }
                        ip_unlock(dp);
                    }
                }
                state = SlowSend;
            }

            // --------------------------------------------------------------
            //  slow_send
            // --------------------------------------------------------------
            SlowSend => {
                let mr = ipc_mqueue_send(
                    kmsg,
                    MACH_MSG_OPTION_NONE,
                    MACH_MSG_TIMEOUT_NONE,
                );
                if mr != MACH_MSG_SUCCESS {
                    let mr2 =
                        ipc_kmsg_copyout_pseudo(kmsg, space, current_map());
                    crate::kassert!(
                        (*kmsg).ikm_marequest == IMAR_NULL,
                        "kmsg->ikm_marequest == IMAR_NULL"
                    );
                    let _ =
                        ipc_kmsg_put(msg, kmsg, (*kmsg).ikm_header.msgh_size);
                    thread_syscall_return(mr | mr2);
                }
                state = SlowGetRcvPort;
            }

            // --------------------------------------------------------------
            //  slow_get_rcv_port
            // --------------------------------------------------------------
            SlowGetRcvPort => {
                let mut tm: *mut ipc_mqueue = null_mut();
                let mut to: *mut ipc_object = null_mut();
                let mr = ipc_mqueue_copyin(space, rcv_name, &mut tm, &mut to);
                if mr != MACH_MSG_SUCCESS {
                    thread_syscall_return(mr);
                }
                rcv_mqueue = tm;
                rcv_object = to;
                state = SlowReceive;
            }

            // --------------------------------------------------------------
            //  slow_receive
            // --------------------------------------------------------------
            SlowReceive => {
                /* Save state for mach_msg_continue */
                (*saved_recv(self_th)).msg = msg;
                (*saved_recv(self_th)).rcv_size = rcv_size;
                (*saved_recv(self_th)).object = rcv_object;
                (*saved_recv(self_th)).mqueue = rcv_mqueue;

                let mut tk: *mut ipc_kmsg_full = null_mut();
                let mut tseq: mach_port_seqno_t = 0;
                let cont: continuation_t = Some(mach_msg_continue_thunk);
                let mr = ipc_mqueue_receive(
                    rcv_mqueue,
                    MACH_MSG_OPTION_NONE,
                    MACH_MSG_SIZE_MAX,
                    MACH_MSG_TIMEOUT_NONE,
                    0, /* FALSE */
                    cont,
                    &mut tk,
                    &mut tseq,
                );
                ipc_object_release(rcv_object);
                if mr != MACH_MSG_SUCCESS {
                    thread_syscall_return(mr);
                }
                kmsg = tk;
                (*kmsg).ikm_header.msgh_seqno = tseq;
                dest_port =
                    (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t;
                state = FastCopyout;
            }

            // --------------------------------------------------------------
            //  slow_copyout
            // --------------------------------------------------------------
            SlowCopyout => {
                reply_size = (*kmsg).ikm_header.msgh_size;
                if (rcv_size as usize) < msg_usize(&(*kmsg).ikm_header) {
                    ipc_kmsg_copyout_dest(kmsg, space);
                    let _ = ipc_kmsg_put(
                        msg,
                        kmsg,
                        core::mem::size_of::<mach_msg_user_header_t>()
                            as mach_msg_size_t,
                    );
                    thread_syscall_return(MACH_RCV_TOO_LARGE);
                }
                let mr = ipc_kmsg_copyout(
                    kmsg,
                    space,
                    current_map(),
                    MACH_PORT_NAME_NULL,
                );
                if mr != MACH_MSG_SUCCESS {
                    if (mr & !MACH_MSG_MASK) == MACH_RCV_BODY_ERROR {
                        let _ = ipc_kmsg_put(
                            msg,
                            kmsg,
                            (*kmsg).ikm_header.msgh_size,
                        );
                    } else {
                        ipc_kmsg_copyout_dest(kmsg, space);
                        let _ = ipc_kmsg_put(
                            msg,
                            kmsg,
                            core::mem::size_of::<mach_msg_user_header_t>()
                                as mach_msg_size_t,
                        );
                    }
                    thread_syscall_return(mr);
                }
                state = FastPut;
            }

            // --------------------------------------------------------------
            //  slow_put
            // --------------------------------------------------------------
            SlowPut => {
                let mr = ipc_kmsg_put(msg, kmsg, reply_size);
                thread_syscall_return(mr);
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  mach_msg_trap — dispatcher.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_msg_trap(
    msg: *mut mach_msg_user_header_t,
    option: mach_msg_option_t,
    send_size: mach_msg_size_t,
    rcv_size: mach_msg_size_t,
    rcv_name: mach_port_name_t,
    time_out: mach_msg_timeout_t,
    notify: mach_port_name_t,
) -> mach_msg_return_t {
    if option == (MACH_SEND_MSG | MACH_RCV_MSG) {
        mach_msg_trap_combined(msg, send_size, rcv_size, rcv_name);
        /* never returns */
    }

    if option == MACH_MSG_OPTION_NONE {
        thread_syscall_return(MACH_MSG_SUCCESS);
    }

    if (option & MACH_SEND_MSG) != 0 {
        let mr = mach_msg_send(msg, option, send_size, time_out, notify);
        if mr != MACH_MSG_SUCCESS {
            return mr;
        }
    }

    if (option & MACH_RCV_MSG) != 0 {
        let mr = mach_msg_receive(msg, option, rcv_size, rcv_name, time_out, notify);
        if mr != MACH_MSG_SUCCESS {
            return mr;
        }
    }

    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  mach_msg_interrupt
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn mach_msg_interrupt(thread: ipc_thread_t) -> boolean_t {
    let mqueue = (*saved_recv(thread)).mqueue;

    imq_lock(mqueue);
    if (*thread).ith_state != MACH_RCV_IN_PROGRESS {
        imq_unlock(mqueue);
        return 0;
    }
    ipc_thread_rmqueue(addr_of_mut!((*mqueue).imq_threads), thread);
    imq_unlock(mqueue);

    ipc_object_release((*saved_recv(thread)).object);

    thread_set_syscall_return(thread, MACH_RCV_INTERRUPTED);
    (*thread).swap_func = Some(thread_exception_return);
    1
}
