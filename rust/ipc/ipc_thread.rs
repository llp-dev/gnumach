//! Port of `ipc/ipc_thread.c`.
//!
//! Three doubly-linked-list helpers operating on the `ith_next`/`ith_prev`
//! fields of `struct thread`.  The list is circular: a singleton element
//! has next == prev == self.

use crate::mach_types::{ipc_thread_queue, ipc_thread_t, thread, ITH_NULL};

/// `ipc_thread_links_init(thread)`: make a singleton circular list.
#[inline]
unsafe fn ipc_thread_links_init(t: *mut thread) {
    (*t).ith_next = t;
    (*t).ith_prev = t;
}

/// `ipc_thread_enqueue` — append a thread to the tail of `queue`.
#[no_mangle]
pub unsafe extern "C" fn ipc_thread_enqueue(
    queue: *mut ipc_thread_queue,
    th: ipc_thread_t,
) {
    let first: ipc_thread_t = (*queue).ithq_base;

    if first == ITH_NULL {
        // Empty queue: th becomes the sole element.  Caller must have called
        // ipc_thread_links_init beforehand (asserted in the C original).
        (*queue).ithq_base = th;
    } else {
        let last: ipc_thread_t = (*first).ith_prev;
        (*th).ith_next = first;
        (*th).ith_prev = last;
        (*first).ith_prev = th;
        (*last).ith_next = th;
        (*queue).ithq_base = th;
    }
}

/// `ipc_thread_dequeue` — pop the head of `queue`.  Returns `ITH_NULL` if
/// the queue was empty.
#[no_mangle]
pub unsafe extern "C" fn ipc_thread_dequeue(
    queue: *mut ipc_thread_queue,
) -> ipc_thread_t {
    let first: ipc_thread_t = (*queue).ithq_base;

    if first == ITH_NULL {
        return ITH_NULL;
    }

    // Inline of ipc_thread_rmqueue_first_macro.
    let next: ipc_thread_t = (*first).ith_next;
    if next == first {
        // Singleton: queue becomes empty.
        (*queue).ithq_base = ITH_NULL;
    } else {
        let prev: ipc_thread_t = (*first).ith_prev;
        (*queue).ithq_base = next;
        (*next).ith_prev = prev;
        (*prev).ith_next = next;
        ipc_thread_links_init(first);
    }

    first
}

/// `ipc_thread_rmqueue` — remove a specific thread from `queue`.
#[no_mangle]
pub unsafe extern "C" fn ipc_thread_rmqueue(
    queue: *mut ipc_thread_queue,
    th: ipc_thread_t,
) {
    // assert(queue->ithq_base != ITH_NULL);

    let next: ipc_thread_t = (*th).ith_next;
    let prev: ipc_thread_t = (*th).ith_prev;

    if next == th {
        // Singleton.  assert(prev == thread); assert(queue->ithq_base == thread);
        (*queue).ithq_base = ITH_NULL;
    } else {
        if (*queue).ithq_base == th {
            (*queue).ithq_base = next;
        }
        (*next).ith_prev = prev;
        (*prev).ith_next = next;
        ipc_thread_links_init(th);
    }
}
