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
    copyout, kmem_cache_free, percpu_array, thread_exception_return,
    thread_set_syscall_return, thread_syscall_return,
};
use crate::ipc_kmsg::{
    ipc_kmsg_copyin, ipc_kmsg_copyout, ipc_kmsg_copyout_dest,
    ipc_kmsg_copyout_pseudo, ipc_kmsg_get, ipc_kmsg_put,
};
use crate::ipc_marequest::ipc_marequest_create;
use crate::ipc_mqueue::{ipc_mqueue_copyin, ipc_mqueue_receive, ipc_mqueue_send};
use crate::ipc_object::ipc_object_release;
use crate::ipc_thread::ipc_thread_rmqueue;
use crate::mach_types::{
    boolean_t, ipc_kmsg_full, ipc_mqueue, ipc_object, ipc_port_t, ipc_space_t,
    ipc_thread_t, kern_return_t, mach_msg_header_t, mach_msg_option_t,
    mach_msg_return_t, mach_msg_size_t, mach_msg_timeout_t,
    mach_msg_user_header_t, mach_port_name_t, mach_port_seqno_t, vm_map_t,
    IMAR_NULL, MACH_MSG_MASK, MACH_MSG_OPTION_NONE, MACH_MSG_SIZE_MAX,
    MACH_MSG_SUCCESS, MACH_MSG_TIMEOUT_NONE, MACH_PORT_NAME_NULL,
    MACH_RCV_BODY_ERROR, MACH_RCV_INTERRUPTED, MACH_RCV_INVALID_NOTIFY,
    MACH_RCV_IN_PROGRESS, MACH_RCV_LARGE, MACH_RCV_MSG, MACH_RCV_NOTIFY,
    MACH_RCV_TIMEOUT, MACH_RCV_TOO_LARGE, MACH_SEND_CANCEL,
    MACH_SEND_INVALID_NOTIFY, MACH_SEND_MSG, MACH_SEND_NOTIFY,
    MACH_SEND_TIMEOUT, MACH_SEND_TIMED_OUT, MACH_SEND_WILL_NOTIFY,
    OFFSETOF_PERCPU_ACTIVE_THREAD, OFFSETOF_TASK_ITK_SPACE, OFFSETOF_TASK_MAP,
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

#[inline]
unsafe fn imq_lock(_mq: *mut ipc_mqueue) {}
#[inline]
unsafe fn imq_unlock(_mq: *mut ipc_mqueue) {}

#[inline]
unsafe fn msg_usize(hdr: *const mach_msg_header_t) -> usize {
    (*hdr).msgh_size as usize
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
//  mach_msg_trap — simplified (no optimised RPC fast path).
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
