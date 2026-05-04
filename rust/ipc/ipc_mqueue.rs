//! Port of `ipc/ipc_mqueue.c`.
//!
//! Functions to manipulate IPC message queues.

use core::ptr::{addr_of_mut, null_mut};

use crate::extern_c::{
    ipc_kobject_server, percpu_array, thread_block, thread_go, thread_will_wait,
    thread_will_wait_with_timeout,
};
use crate::ipc_kmsg::{ipc_kmsg_dequeue, ipc_kmsg_destroy, ipc_kmsg_enqueue, ipc_kmsg_rmqueue};
use crate::ipc_marequest::ipc_marequest_destroy;
use crate::ipc_object::ipc_object_release;
use crate::ipc_pset::ipc_pset_remove;
use crate::ipc_thread::{ipc_thread_dequeue, ipc_thread_enqueue, ipc_thread_rmqueue};
use crate::mach_types::{
    boolean_t, continuation_t, ipc_entry_t, ipc_kmsg_full, ipc_kmsg_queue, ipc_marequest_t,
    ipc_mqueue, ipc_object, ipc_port_t, ipc_pset_t, ipc_space_t, ipc_thread_queue, ipc_thread_t,
    long_natural_t, mach_msg_header_t, mach_msg_option_t, mach_msg_return_t, mach_msg_size_t,
    mach_msg_timeout_t, mach_port_name_t, mach_port_seqno_t, IE_BITS_TYPE_MASK, IE_NULL, IKM_NULL,
    IMAR_NULL, IPS_NULL, ITH_NULL, KERN_SUCCESS, MACH_MSGH_BITS_CIRCULAR, MACH_MSG_SUCCESS,
    MACH_MSG_TYPE_PORT_SEND_ONCE, MACH_PORT_TYPE_PORT_SET, MACH_PORT_TYPE_RECEIVE,
    MACH_RCV_INTERRUPTED, MACH_RCV_INVALID_NAME, MACH_RCV_IN_PROGRESS, MACH_RCV_IN_SET,
    MACH_RCV_PORT_CHANGED, MACH_RCV_PORT_DIED, MACH_RCV_TIMED_OUT, MACH_RCV_TIMEOUT,
    MACH_RCV_TOO_LARGE, MACH_SEND_ALWAYS, MACH_SEND_INTERRUPTED, MACH_SEND_IN_PROGRESS,
    MACH_SEND_TIMED_OUT, MACH_SEND_TIMEOUT, OFFSETOF_PERCPU_ACTIVE_THREAD,
    OFFSETOF_TASK_MESSAGES_RECEIVED, OFFSETOF_TASK_MESSAGES_SENT, THREAD_INTERRUPTED,
    THREAD_RESTART, THREAD_TIMED_OUT,
};

// ---------------------------------------------------------------------------
//  Macros expanded inline (1:1 with the C originals)
// ---------------------------------------------------------------------------

use crate::locks::{
    imq_lock, imq_lock_init, imq_unlock, io_reference, io_unlock, ip_active, ip_check_unlock,
    ip_lock, ip_release, ip_unlock, ips_active, ips_lock, ips_unlock, is_read_lock, is_read_unlock,
};

#[inline]
unsafe fn ips_check_unlock(pset: ipc_pset_t) {
    crate::locks::io_check_unlock(addr_of_mut!((*pset).ips_target.ipt_object));
}

/// `ipc_kmsg_queue_init(q)` — C macro: `(q)->ikmq_base = IKM_NULL;`.
#[inline]
unsafe fn ipc_kmsg_queue_init(q: *mut ipc_kmsg_queue) {
    (*q).ikmq_base = null_mut();
}

/// `ipc_thread_queue_init(q)` — C macro: `(q)->ithq_base = ITH_NULL;`.
#[inline]
unsafe fn ipc_thread_queue_init(q: *mut ipc_thread_queue) {
    (*q).ithq_base = ITH_NULL;
}

#[inline]
unsafe fn ipc_kmsg_queue_first(q: *mut ipc_kmsg_queue) -> *mut ipc_kmsg_full {
    (*q).ikmq_base as *mut ipc_kmsg_full
}

#[inline]
unsafe fn ipc_kmsg_queue_next(
    q: *mut ipc_kmsg_queue,
    kmsg: *mut ipc_kmsg_full,
) -> *mut ipc_kmsg_full {
    let next = (*kmsg).ikm_next;
    if next == (*q).ikmq_base as *mut ipc_kmsg_full {
        null_mut()
    } else {
        next
    }
}

#[inline]
unsafe fn ipc_thread_queue_first(q: *mut ipc_thread_queue) -> ipc_thread_t {
    (*q).ithq_base
}

/// `MACH_MSGH_BITS_REMOTE(bits) = bits & 0xff`.
#[inline]
fn mach_msgh_bits_remote(bits: u32) -> u32 {
    bits & 0x0000_00ff
}

/// `msg_usize(hdr) = hdr->msgh_size` from `ipc/copy_user.h`.
#[inline]
unsafe fn msg_usize(hdr: *const mach_msg_header_t) -> usize {
    (*hdr).msgh_size as usize
}

/// `current_thread()` — `percpu_array[0].active_thread`.
#[inline]
unsafe fn current_thread() -> ipc_thread_t {
    let base = addr_of_mut!(percpu_array) as *mut u8;
    *(base.add(OFFSETOF_PERCPU_ACTIVE_THREAD) as *mut ipc_thread_t)
}

/// `current_task()->messages_sent++;`.  The task struct is opaque on the
/// Rust side; the offset is pinned by `_Static_assert` in
/// `ipc/ipc_layout_asserts.c`.
#[inline]
unsafe fn current_task_inc_messages_sent() {
    let task = (*current_thread()).task as *mut u8;
    let p = task.add(OFFSETOF_TASK_MESSAGES_SENT) as *mut long_natural_t;
    *p = (*p).wrapping_add(1);
}

#[inline]
unsafe fn current_task_inc_messages_received() {
    let task = (*current_thread()).task as *mut u8;
    let p = task.add(OFFSETOF_TASK_MESSAGES_RECEIVED) as *mut long_natural_t;
    *p = (*p).wrapping_add(1);
}

/// Mirror of the static inline `ipc_entry_lookup` from `ipc/ipc_space.h`.
#[inline]
unsafe fn ipc_entry_lookup(space: ipc_space_t, name: mach_port_name_t) -> ipc_entry_t {
    let entry = crate::extern_c::rdxtree_lookup_common(
        core::ptr::addr_of!((*space).is_map),
        name as crate::mach_types::rdxtree_key_t,
        0,
    ) as ipc_entry_t;
    if entry == IE_NULL {
        return IE_NULL;
    }
    if (*entry).ie_bits & IE_BITS_TYPE_MASK == 0 {
        return IE_NULL;
    }
    entry
}

// ---------------------------------------------------------------------------
//  ipc_mqueue_init
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_mqueue_init(mqueue: *mut ipc_mqueue) {
    imq_lock_init(mqueue);
    ipc_kmsg_queue_init(addr_of_mut!((*mqueue).imq_messages));
    ipc_thread_queue_init(addr_of_mut!((*mqueue).imq_threads));
}

// ---------------------------------------------------------------------------
//  ipc_mqueue_move
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_mqueue_move(
    dest: *mut ipc_mqueue,
    source: *mut ipc_mqueue,
    port: ipc_port_t,
) {
    let oldq = addr_of_mut!((*source).imq_messages);
    let newq = addr_of_mut!((*dest).imq_messages);
    let blockedq = addr_of_mut!((*dest).imq_threads);

    let mut kmsg = ipc_kmsg_queue_first(oldq);
    while !kmsg.is_null() {
        let next = ipc_kmsg_queue_next(oldq, kmsg);

        /* only move messages sent to port */
        if (*kmsg).ikm_header.msgh_remote_port != (port as usize as u32) {
            kmsg = next;
            continue;
        }

        ipc_kmsg_rmqueue(oldq, kmsg);

        /* before adding kmsg to newq, check for a blocked receiver */
        let mut delivered = false;
        loop {
            let th = ipc_thread_dequeue(blockedq);
            if th == ITH_NULL {
                break;
            }
            crate::kassert!((*newq).ikmq_base.is_null(), "ipc_kmsg_queue_empty(newq)");
            thread_go(th);

            /* check if the receiver can handle the message */
            if (*kmsg).ikm_header.msgh_size <= (*th).data.msize {
                (*th).ith_state = MACH_MSG_SUCCESS;
                (*th).data.kmsg = kmsg as *mut crate::mach_types::ipc_kmsg;
                (*th).ith_seqno = (*port).ip_seqno;
                (*port).ip_seqno = (*port).ip_seqno.wrapping_add(1);

                delivered = true;
                break;
            }

            (*th).ith_state = MACH_RCV_TOO_LARGE;
            (*th).data.msize = (*kmsg).ikm_header.msgh_size;
        }

        if !delivered {
            /* didn't find a receiver to handle the message */
            ipc_kmsg_enqueue(newq, kmsg);
        }
        kmsg = next;
    }
}

// ---------------------------------------------------------------------------
//  ipc_mqueue_changed
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_mqueue_changed(mqueue: *mut ipc_mqueue, mr: mach_msg_return_t) {
    loop {
        let th = ipc_thread_dequeue(addr_of_mut!((*mqueue).imq_threads));
        if th == ITH_NULL {
            break;
        }
        (*th).ith_state = mr;
        thread_go(th);
    }
}

// ---------------------------------------------------------------------------
//  ipc_mqueue_send
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_mqueue_send(
    kmsg: *mut ipc_kmsg_full,
    option: mach_msg_option_t,
    mut time_out: mach_msg_timeout_t,
) -> mach_msg_return_t {
    let port: ipc_port_t = (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t;
    crate::kassert!(
        (port as usize) != 0 && (port as usize) != !0usize,
        "IP_VALID(port)"
    );

    ip_lock(port);

    if (*port).data.receiver == crate::ipc_space::ipc_space_kernel {
        /*
         *  We can check ip_receiver == ipc_space_kernel before checking
         *  that the port is active because ipc_port_dealloc_kernel clears
         *  ip_receiver before destroying a kernel port.
         */
        crate::kassert!(ip_active(port), "ip_active(port)");
        ip_unlock(port);

        let reply = ipc_kobject_server(kmsg);
        if reply != IKM_NULL {
            /* ipc_mqueue_send_always(reply) — recurse via the same fn. */
            let _ = ipc_mqueue_send(reply, MACH_SEND_ALWAYS, 0);
        }
        return MACH_MSG_SUCCESS;
    }

    'outer: loop {
        /*
         *  Can't deliver to a dead port.  However, we can pretend it got
         *  sent and was then immediately destroyed.
         */
        if !ip_active(port) {
            /*
             *  Don't let ipc_kmsg_destroy deallocate the port right;
             *  might end up in an infinite loop trying to deliver a
             *  send-once notification.
             */
            ip_release(port);
            ip_check_unlock(port);
            (*kmsg).ikm_header.msgh_remote_port = 0; /* MACH_PORT_NULL */
            ipc_kmsg_destroy(kmsg);
            return MACH_MSG_SUCCESS;
        }

        /*
         *  Don't block if:
         *    1) We're under the queue limit.
         *    2) Caller used the MACH_SEND_ALWAYS internal option.
         *    3) Message is sent to a send-once right.
         */
        if (*port).ip_msgcount < (*port).ip_qlimit
            || (option & MACH_SEND_ALWAYS) != 0
            || mach_msgh_bits_remote((*kmsg).ikm_header.msgh_bits) == MACH_MSG_TYPE_PORT_SEND_ONCE
        {
            break 'outer;
        }

        /* must block waiting for queue to clear */
        let self_th = current_thread();

        if (option & MACH_SEND_TIMEOUT) != 0 {
            if time_out == 0 {
                ip_unlock(port);
                return MACH_SEND_TIMED_OUT;
            }
            thread_will_wait_with_timeout(self_th, time_out);
        } else {
            thread_will_wait(self_th);
        }

        ipc_thread_enqueue(addr_of_mut!((*port).ip_blocked), self_th);
        (*self_th).ith_state = MACH_SEND_IN_PROGRESS;

        ip_unlock(port);
        thread_block(None); /* thread_no_continuation */
        ip_lock(port);

        /* why did we wake up? */
        if (*self_th).ith_state == MACH_MSG_SUCCESS {
            continue 'outer;
        }
        crate::kassert!(
            (*self_th).ith_state == MACH_SEND_IN_PROGRESS,
            "self->ith_state == MACH_SEND_IN_PROGRESS"
        );

        /* take ourselves off blocked queue */
        ipc_thread_rmqueue(addr_of_mut!((*port).ip_blocked), self_th);

        /* Thread wakeup-reason field tells us why the wait was
         * interrupted. */
        match (*self_th).wait_result {
            x if x == THREAD_INTERRUPTED => {
                /* send was interrupted - give up */
                ip_unlock(port);
                return MACH_SEND_INTERRUPTED;
            }
            x if x == THREAD_TIMED_OUT => {
                /* timeout expired */
                crate::kassert!(
                    (option & MACH_SEND_TIMEOUT) != 0,
                    "option & MACH_SEND_TIMEOUT"
                );
                time_out = 0;
            }
            _ => {
                /* THREAD_RESTART or unexpected */
                crate::kpanic!("ipc_mqueue_send: strange wait_result");
            }
        }
    }

    if ((*kmsg).ikm_header.msgh_bits & MACH_MSGH_BITS_CIRCULAR) != 0 {
        ip_unlock(port);
        /* don't allow the creation of a circular loop */
        ipc_kmsg_destroy(kmsg);
        return MACH_MSG_SUCCESS;
    }

    {
        (*port).ip_msgcount += 1;
        crate::kassert!((*port).ip_msgcount > 0, "port->ip_msgcount > 0");

        let pset = (*port).ip_pset;
        let mqueue: *mut ipc_mqueue = if pset == IPS_NULL {
            addr_of_mut!((*port).ip_target.ipt_messages)
        } else {
            addr_of_mut!((*pset).ips_target.ipt_messages)
        };

        imq_lock(mqueue);
        let receivers = addr_of_mut!((*mqueue).imq_threads);

        /*
         *  Can unlock the port now that the msg queue is locked and we
         *  know the port is active.  While the msg queue is locked, we
         *  have control of the kmsg, so the ref in it for the port is
         *  still good.
         */
        ip_unlock(port);

        /* check for a receiver for the message */
        loop {
            let receiver = ipc_thread_queue_first(receivers);
            if receiver == ITH_NULL {
                /* no receivers; queue kmsg */
                ipc_kmsg_enqueue(addr_of_mut!((*mqueue).imq_messages), kmsg);
                imq_unlock(mqueue);
                break;
            }

            /* ipc_thread_rmqueue_first_macro is the same as
             * ipc_thread_dequeue (drop the head we just inspected). */
            ipc_thread_dequeue(receivers);
            crate::kassert!(
                (*mqueue).imq_messages.ikmq_base.is_null(),
                "ipc_kmsg_queue_empty(&mqueue->imq_messages)"
            );

            if (*kmsg).ikm_header.msgh_size <= (*receiver).data.msize {
                /* got a successful receiver */
                (*receiver).ith_state = MACH_MSG_SUCCESS;
                (*receiver).data.kmsg = kmsg as *mut crate::mach_types::ipc_kmsg;
                (*receiver).ith_seqno = (*port).ip_seqno;
                (*port).ip_seqno = (*port).ip_seqno.wrapping_add(1);
                imq_unlock(mqueue);

                thread_go(receiver);
                break;
            }

            (*receiver).ith_state = MACH_RCV_TOO_LARGE;
            (*receiver).data.msize = (*kmsg).ikm_header.msgh_size;
            thread_go(receiver);
        }
    }

    current_task_inc_messages_sent();
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_mqueue_copyin
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_mqueue_copyin(
    space: ipc_space_t,
    name: mach_port_name_t,
    mqueuep: *mut *mut ipc_mqueue,
    objectp: *mut *mut ipc_object,
) -> mach_msg_return_t {
    is_read_lock(space);
    if (*space).is_active == 0 {
        is_read_unlock(space);
        return MACH_RCV_INVALID_NAME;
    }

    let entry = ipc_entry_lookup(space, name);
    if entry == IE_NULL {
        is_read_unlock(space);
        return MACH_RCV_INVALID_NAME;
    }

    let bits = (*entry).ie_bits;
    let object = (*entry).ie_object;

    let mqueue: *mut ipc_mqueue;
    if (bits & MACH_PORT_TYPE_RECEIVE) != 0 {
        let port = object as ipc_port_t;
        crate::kassert!(!port.is_null(), "port != IP_NULL");

        ip_lock(port);
        crate::kassert!(ip_active(port), "ip_active(port)");
        crate::kassert!(
            (*port).ip_target.ipt_name == name,
            "port->ip_receiver_name == name"
        );
        crate::kassert!((*port).data.receiver == space, "port->ip_receiver == space");
        is_read_unlock(space);

        let pset = (*port).ip_pset;
        if pset != IPS_NULL {
            ips_lock(pset);
            if ips_active(pset) {
                ips_unlock(pset);
                ip_unlock(port);
                return MACH_RCV_IN_SET;
            }

            ipc_pset_remove(pset, port);
            ips_check_unlock(pset);
            crate::kassert!((*port).ip_pset == IPS_NULL, "port->ip_pset == IPS_NULL");
        }

        mqueue = addr_of_mut!((*port).ip_target.ipt_messages);
    } else if (bits & MACH_PORT_TYPE_PORT_SET) != 0 {
        let pset = object as ipc_pset_t;
        crate::kassert!(pset != IPS_NULL, "pset != IPS_NULL");

        ips_lock(pset);
        crate::kassert!(ips_active(pset), "ips_active(pset)");
        crate::kassert!(
            (*pset).ips_target.ipt_name == name,
            "pset->ips_local_name == name"
        );
        is_read_unlock(space);

        mqueue = addr_of_mut!((*pset).ips_target.ipt_messages);
    } else {
        is_read_unlock(space);
        return MACH_RCV_INVALID_NAME;
    }

    /*
     *  At this point, the object is locked and active, the space is
     *  unlocked, and mqueue is initialized.
     */
    io_reference(object);
    imq_lock(mqueue);
    io_unlock(object);

    *objectp = object;
    *mqueuep = mqueue;
    MACH_MSG_SUCCESS
}

// ---------------------------------------------------------------------------
//  ipc_mqueue_receive
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_mqueue_receive(
    mqueue: *mut ipc_mqueue,
    option: mach_msg_option_t,
    max_size: mach_msg_size_t,
    mut time_out: mach_msg_timeout_t,
    resume: boolean_t,
    continuation: continuation_t,
    kmsgp: *mut *mut ipc_kmsg_full,
    seqnop: *mut mach_port_seqno_t,
) -> mach_msg_return_t {
    let port: ipc_port_t;
    let kmsg: *mut ipc_kmsg_full;
    let seqno: mach_port_seqno_t;

    'recv: {
        let kmsgs = addr_of_mut!((*mqueue).imq_messages);
        let self_th = current_thread();

        /* Mirrors the C `goto after_thread_block;` from the resume path. */
        let mut at_after_block: bool = resume != 0;

        loop {
            if !at_after_block {
                /* === top of C for(;;) — mqueue lock IS held === */
                let head = ipc_kmsg_queue_first(kmsgs);
                if head != IKM_NULL {
                    /* check space requirements */
                    if (msg_usize(addr_of_mut!((*head).ikm_header)) as u32) > max_size {
                        *(kmsgp as *mut mach_msg_size_t) = (*head).ikm_header.msgh_size;
                        imq_unlock(mqueue);
                        return MACH_RCV_TOO_LARGE;
                    }

                    /* ipc_kmsg_rmqueue_first_macro = ipc_kmsg_dequeue. */
                    let _ = ipc_kmsg_dequeue(kmsgs);
                    kmsg = head;
                    port = (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t;
                    seqno = (*port).ip_seqno;
                    (*port).ip_seqno = (*port).ip_seqno.wrapping_add(1);
                    break 'recv;
                }

                /* must block waiting for a message */
                if (option & MACH_RCV_TIMEOUT) != 0 {
                    if time_out == 0 {
                        imq_unlock(mqueue);
                        return MACH_RCV_TIMED_OUT;
                    }
                    thread_will_wait_with_timeout(self_th, time_out);
                } else {
                    thread_will_wait(self_th);
                }

                ipc_thread_enqueue(addr_of_mut!((*mqueue).imq_threads), self_th);
                (*self_th).ith_state = MACH_RCV_IN_PROGRESS;
                (*self_th).data.msize = max_size;

                imq_unlock(mqueue);
                thread_block(continuation);
                /* fall through to after_thread_block */
            }
            at_after_block = false;

            /* === after_thread_block: === */
            imq_lock(mqueue);

            if (*self_th).ith_state == MACH_MSG_SUCCESS {
                /* pick up the message that was handed to us */
                kmsg = (*self_th).data.kmsg as *mut ipc_kmsg_full;
                seqno = (*self_th).ith_seqno;
                port = (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t;
                break 'recv;
            }

            match (*self_th).ith_state {
                x if x == MACH_RCV_TOO_LARGE => {
                    *(kmsgp as *mut mach_msg_size_t) = (*self_th).data.msize;
                    imq_unlock(mqueue);
                    return (*self_th).ith_state;
                }
                x if x == MACH_RCV_PORT_DIED || x == MACH_RCV_PORT_CHANGED => {
                    imq_unlock(mqueue);
                    return (*self_th).ith_state;
                }
                x if x == MACH_RCV_IN_PROGRESS => {
                    ipc_thread_rmqueue(addr_of_mut!((*mqueue).imq_threads), self_th);

                    match (*self_th).wait_result {
                        y if y == THREAD_INTERRUPTED => {
                            imq_unlock(mqueue);
                            return MACH_RCV_INTERRUPTED;
                        }
                        y if y == THREAD_TIMED_OUT => {
                            crate::kassert!(
                                (option & MACH_RCV_TIMEOUT) != 0,
                                "option & MACH_RCV_TIMEOUT"
                            );
                            time_out = 0;
                        }
                        _ => { /* THREAD_RESTART / unexpected — panic. */ }
                    }
                    /* break of inner switch -> continue outer for(;;) */
                }
                _ => {
                    crate::kpanic!("ipc_mqueue_receive: strange ith_state");
                }
            }
            let _ = THREAD_RESTART;
            /* fall through to next iteration of for(;;) — lock still held */
        }
    }

    /* we have a kmsg; unlock the msg queue */
    imq_unlock(mqueue);
    crate::kassert!(
        (msg_usize(addr_of_mut!((*kmsg).ikm_header)) as u32) <= max_size,
        "msg_usize(&kmsg->ikm_header) <= max_size"
    );

    {
        let marequest: ipc_marequest_t = (*kmsg).ikm_marequest;
        if marequest != IMAR_NULL {
            ipc_marequest_destroy(marequest);
            (*kmsg).ikm_marequest = IMAR_NULL;
        }
        crate::kassert!(
            ((*kmsg).ikm_header.msgh_bits & MACH_MSGH_BITS_CIRCULAR) == 0,
            "(kmsg->ikm_header.msgh_bits & MACH_MSGH_BITS_CIRCULAR) == 0"
        );
        crate::kassert!(
            port == (*kmsg).ikm_header.msgh_remote_port as usize as ipc_port_t,
            "port == kmsg->ikm_header.msgh_remote_port"
        );

        ip_lock(port);

        if ip_active(port) {
            crate::kassert!((*port).ip_msgcount > 0, "port->ip_msgcount > 0");
            (*port).ip_msgcount -= 1;

            let senders = addr_of_mut!((*port).ip_blocked);
            let sender = ipc_thread_queue_first(senders);

            if sender != ITH_NULL && (*port).ip_msgcount < (*port).ip_qlimit {
                ipc_thread_rmqueue(senders, sender);
                (*sender).ith_state = MACH_MSG_SUCCESS;
                thread_go(sender);
            }
        }

        ip_unlock(port);
    }

    current_task_inc_messages_received();

    *kmsgp = kmsg;
    *seqnop = seqno;

    let _ = ipc_object_release;
    let _ = (KERN_SUCCESS, MACH_RCV_INVALID_NAME);
    MACH_MSG_SUCCESS
}
