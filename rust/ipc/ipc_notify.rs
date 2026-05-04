//! Port of `ipc/ipc_notify.c`.
//!
//! Six notification-message templates plus six routines that allocate a
//! kmsg, copy the appropriate template into it, fill in the destination /
//! payload, and submit it via `ipc_mqueue_send_always`.

use core::ptr::addr_of_mut;

use crate::extern_c::{kalloc, printf};
use crate::ipc_mqueue::ipc_mqueue_send;
use crate::ipc_port::{ipc_port_release_receive, ipc_port_release_sonce};
use crate::mach_types::{
    ipc_kmsg_full, ipc_port, mach_dead_name_notification_t, mach_msg_accepted_notification_t,
    mach_msg_header_t, mach_msg_type_t, mach_no_senders_notification_t,
    mach_port_deleted_notification_t, mach_port_destroyed_notification_t, mach_port_mscount_t,
    mach_port_name_t, mach_send_once_notification_t, vm_size_t, IKM_NULL, IKM_OVERHEAD, IMAR_NULL,
    MACH_MSGH_BITS, MACH_MSGH_BITS_COMPLEX, MACH_MSG_TIMEOUT_NONE, MACH_MSG_TYPE_INTEGER_32,
    MACH_MSG_TYPE_PORT_NAME, MACH_MSG_TYPE_PORT_RECEIVE, MACH_MSG_TYPE_PORT_SEND_ONCE,
    MACH_NOTIFY_DEAD_NAME, MACH_NOTIFY_MSG_ACCEPTED, MACH_NOTIFY_NO_SENDERS,
    MACH_NOTIFY_PORT_DELETED, MACH_NOTIFY_PORT_DESTROYED, MACH_NOTIFY_SEND_ONCE, MACH_PORT_NULL,
    MACH_SEND_ALWAYS, PORT_NAME_T_SIZE_IN_BITS, PORT_T_SIZE_IN_BITS,
};

const NOTIFY_MSGH_SEQNO: u32 = 0;
const TRUE: u32 = 1;
const FALSE: u32 = 0;

// ---------------------------------------------------------------------------
//  Templates — zero-initialized at link time, populated by ipc_notify_init.
// ---------------------------------------------------------------------------

const ZERO_HEADER: mach_msg_header_t = mach_msg_header_t {
    msgh_bits: 0,
    msgh_size: 0,
    msgh_remote_port: 0,
    msgh_local_port: 0,
    msgh_seqno: 0,
    msgh_id: 0,
};
const ZERO_TYPE: mach_msg_type_t = mach_msg_type_t { bits: 0 };

#[no_mangle]
pub static mut ipc_notify_port_deleted_template: mach_port_deleted_notification_t =
    mach_port_deleted_notification_t {
        not_header: ZERO_HEADER,
        not_type: ZERO_TYPE,
        not_port: 0,
    };

#[no_mangle]
pub static mut ipc_notify_msg_accepted_template: mach_msg_accepted_notification_t =
    mach_msg_accepted_notification_t {
        not_header: ZERO_HEADER,
        not_type: ZERO_TYPE,
        not_port: 0,
    };

#[no_mangle]
pub static mut ipc_notify_port_destroyed_template: mach_port_destroyed_notification_t =
    mach_port_destroyed_notification_t {
        not_header: ZERO_HEADER,
        not_type: ZERO_TYPE,
        not_port: 0,
    };

#[no_mangle]
pub static mut ipc_notify_no_senders_template: mach_no_senders_notification_t =
    mach_no_senders_notification_t {
        not_header: ZERO_HEADER,
        not_type: ZERO_TYPE,
        not_count: 0,
    };

#[no_mangle]
pub static mut ipc_notify_send_once_template: mach_send_once_notification_t =
    mach_send_once_notification_t {
        not_header: ZERO_HEADER,
    };

#[no_mangle]
pub static mut ipc_notify_dead_name_template: mach_dead_name_notification_t =
    mach_dead_name_notification_t {
        not_header: ZERO_HEADER,
        not_type: ZERO_TYPE,
        not_port: 0,
    };

// ---------------------------------------------------------------------------
//  Helpers (file-static in C; module-private in Rust).
// ---------------------------------------------------------------------------

/// `ikm_alloc(size)` — `(ipc_kmsg_t) kalloc(ikm_plus_overhead(size))`.
#[inline]
unsafe fn ikm_alloc(size: vm_size_t) -> *mut ipc_kmsg_full {
    kalloc(size + IKM_OVERHEAD) as *mut ipc_kmsg_full
}

/// `ikm_init(kmsg, size)` — sets `ikm_size = size + overhead` and
/// `ikm_marequest = IMAR_NULL`.
#[inline]
unsafe fn ikm_init(kmsg: *mut ipc_kmsg_full, size: vm_size_t) {
    (*kmsg).ikm_size = size + IKM_OVERHEAD;
    (*kmsg).ikm_marequest = IMAR_NULL;
}

/// `ipc_mqueue_send_always(kmsg)` — `(void) ipc_mqueue_send(kmsg,
///  MACH_SEND_ALWAYS, MACH_MSG_TIMEOUT_NONE)`.
#[inline]
unsafe fn ipc_mqueue_send_always(kmsg: *mut ipc_kmsg_full) {
    let _ = ipc_mqueue_send(kmsg, MACH_SEND_ALWAYS, MACH_MSG_TIMEOUT_NONE);
}

unsafe fn ipc_notify_init_port_deleted(n: *mut mach_port_deleted_notification_t) {
    let m = addr_of_mut!((*n).not_header);
    let t = addr_of_mut!((*n).not_type);

    (*m).msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
    (*m).msgh_size = core::mem::size_of::<mach_port_deleted_notification_t>() as u32;
    (*m).msgh_seqno = NOTIFY_MSGH_SEQNO;
    (*m).msgh_local_port = MACH_PORT_NULL;
    (*m).msgh_remote_port = MACH_PORT_NULL;
    (*m).msgh_id = MACH_NOTIFY_PORT_DELETED;

    *t = mach_msg_type_t::new(
        MACH_MSG_TYPE_PORT_NAME,
        PORT_NAME_T_SIZE_IN_BITS,
        1,
        TRUE,
        FALSE,
        FALSE,
        0,
    );

    (*n).not_port = MACH_PORT_NULL;
}

unsafe fn ipc_notify_init_msg_accepted(n: *mut mach_msg_accepted_notification_t) {
    let m = addr_of_mut!((*n).not_header);
    let t = addr_of_mut!((*n).not_type);

    (*m).msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
    (*m).msgh_size = core::mem::size_of::<mach_msg_accepted_notification_t>() as u32;
    (*m).msgh_seqno = NOTIFY_MSGH_SEQNO;
    (*m).msgh_local_port = MACH_PORT_NULL;
    (*m).msgh_remote_port = MACH_PORT_NULL;
    (*m).msgh_id = MACH_NOTIFY_MSG_ACCEPTED;

    *t = mach_msg_type_t::new(
        MACH_MSG_TYPE_PORT_NAME,
        PORT_NAME_T_SIZE_IN_BITS,
        1,
        TRUE,
        FALSE,
        FALSE,
        0,
    );

    (*n).not_port = MACH_PORT_NULL;
}

unsafe fn ipc_notify_init_port_destroyed(n: *mut mach_port_destroyed_notification_t) {
    let m = addr_of_mut!((*n).not_header);
    let t = addr_of_mut!((*n).not_type);

    (*m).msgh_bits = MACH_MSGH_BITS_COMPLEX | MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
    (*m).msgh_size = core::mem::size_of::<mach_port_destroyed_notification_t>() as u32;
    (*m).msgh_seqno = NOTIFY_MSGH_SEQNO;
    (*m).msgh_local_port = MACH_PORT_NULL;
    (*m).msgh_remote_port = MACH_PORT_NULL;
    (*m).msgh_id = MACH_NOTIFY_PORT_DESTROYED;

    *t = mach_msg_type_t::new(
        MACH_MSG_TYPE_PORT_RECEIVE,
        PORT_T_SIZE_IN_BITS,
        1,
        TRUE,
        FALSE,
        FALSE,
        0,
    );

    (*n).not_port = MACH_PORT_NULL;
}

unsafe fn ipc_notify_init_no_senders(n: *mut mach_no_senders_notification_t) {
    let m = addr_of_mut!((*n).not_header);
    let t = addr_of_mut!((*n).not_type);

    (*m).msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
    (*m).msgh_size = core::mem::size_of::<mach_no_senders_notification_t>() as u32;
    (*m).msgh_seqno = NOTIFY_MSGH_SEQNO;
    (*m).msgh_local_port = MACH_PORT_NULL;
    (*m).msgh_remote_port = MACH_PORT_NULL;
    (*m).msgh_id = MACH_NOTIFY_NO_SENDERS;

    *t = mach_msg_type_t::new(MACH_MSG_TYPE_INTEGER_32, 32, 1, TRUE, FALSE, FALSE, 0);

    (*n).not_count = 0;
}

unsafe fn ipc_notify_init_send_once(n: *mut mach_send_once_notification_t) {
    let m = addr_of_mut!((*n).not_header);

    (*m).msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
    (*m).msgh_size = core::mem::size_of::<mach_send_once_notification_t>() as u32;
    (*m).msgh_seqno = NOTIFY_MSGH_SEQNO;
    (*m).msgh_local_port = MACH_PORT_NULL;
    (*m).msgh_remote_port = MACH_PORT_NULL;
    (*m).msgh_id = MACH_NOTIFY_SEND_ONCE;
}

unsafe fn ipc_notify_init_dead_name(n: *mut mach_dead_name_notification_t) {
    let m = addr_of_mut!((*n).not_header);
    let t = addr_of_mut!((*n).not_type);

    (*m).msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_PORT_SEND_ONCE, 0);
    (*m).msgh_size = core::mem::size_of::<mach_dead_name_notification_t>() as u32;
    (*m).msgh_seqno = NOTIFY_MSGH_SEQNO;
    (*m).msgh_local_port = MACH_PORT_NULL;
    (*m).msgh_remote_port = MACH_PORT_NULL;
    (*m).msgh_id = MACH_NOTIFY_DEAD_NAME;

    *t = mach_msg_type_t::new(
        MACH_MSG_TYPE_PORT_NAME,
        PORT_NAME_T_SIZE_IN_BITS,
        1,
        TRUE,
        FALSE,
        FALSE,
        0,
    );

    (*n).not_port = MACH_PORT_NULL;
}

// ---------------------------------------------------------------------------
//  Public init + senders.
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_init() {
    ipc_notify_init_port_deleted(addr_of_mut!(ipc_notify_port_deleted_template));
    ipc_notify_init_msg_accepted(addr_of_mut!(ipc_notify_msg_accepted_template));
    ipc_notify_init_port_destroyed(addr_of_mut!(ipc_notify_port_destroyed_template));
    ipc_notify_init_no_senders(addr_of_mut!(ipc_notify_no_senders_template));
    ipc_notify_init_send_once(addr_of_mut!(ipc_notify_send_once_template));
    ipc_notify_init_dead_name(addr_of_mut!(ipc_notify_dead_name_template));
}

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_port_deleted(port: *mut ipc_port, name: mach_port_name_t) {
    let kmsg: *mut ipc_kmsg_full;
    let n: *mut mach_port_deleted_notification_t;

    kmsg = ikm_alloc(core::mem::size_of::<mach_port_deleted_notification_t>() as u32);
    if kmsg == IKM_NULL {
        printf(
            b"dropped port-deleted (0x%p, 0x%x)\n\0".as_ptr() as *const _,
            port,
            name,
        );
        ipc_port_release_sonce(port);
        return;
    }

    ikm_init(
        kmsg,
        core::mem::size_of::<mach_port_deleted_notification_t>() as u32,
    );
    n = addr_of_mut!((*kmsg).ikm_header) as *mut mach_port_deleted_notification_t;
    *n = core::ptr::read(addr_of_mut!(ipc_notify_port_deleted_template));

    (*n).not_header.msgh_remote_port = port as u32;
    (*n).not_port = name;

    ipc_mqueue_send_always(kmsg);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_msg_accepted(port: *mut ipc_port, name: mach_port_name_t) {
    let kmsg: *mut ipc_kmsg_full;
    let n: *mut mach_msg_accepted_notification_t;

    kmsg = ikm_alloc(core::mem::size_of::<mach_msg_accepted_notification_t>() as u32);
    if kmsg == IKM_NULL {
        printf(
            b"dropped msg-accepted (0x%p, 0x%x)\n\0".as_ptr() as *const _,
            port,
            name,
        );
        ipc_port_release_sonce(port);
        return;
    }

    ikm_init(
        kmsg,
        core::mem::size_of::<mach_msg_accepted_notification_t>() as u32,
    );
    n = addr_of_mut!((*kmsg).ikm_header) as *mut mach_msg_accepted_notification_t;
    *n = core::ptr::read(addr_of_mut!(ipc_notify_msg_accepted_template));

    (*n).not_header.msgh_remote_port = port as u32;
    (*n).not_port = name;

    ipc_mqueue_send_always(kmsg);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_port_destroyed(port: *mut ipc_port, right: *mut ipc_port) {
    let kmsg: *mut ipc_kmsg_full;
    let n: *mut mach_port_destroyed_notification_t;

    kmsg = ikm_alloc(core::mem::size_of::<mach_port_destroyed_notification_t>() as u32);
    if kmsg == IKM_NULL {
        printf(
            b"dropped port-destroyed (0x%p, 0x%p)\n\0".as_ptr() as *const _,
            port,
            right,
        );
        ipc_port_release_sonce(port);
        ipc_port_release_receive(right);
        return;
    }

    ikm_init(
        kmsg,
        core::mem::size_of::<mach_port_destroyed_notification_t>() as u32,
    );
    n = addr_of_mut!((*kmsg).ikm_header) as *mut mach_port_destroyed_notification_t;
    *n = core::ptr::read(addr_of_mut!(ipc_notify_port_destroyed_template));

    (*n).not_header.msgh_remote_port = port as u32;
    (*n).not_port = right as u32;

    ipc_mqueue_send_always(kmsg);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_no_senders(port: *mut ipc_port, mscount: mach_port_mscount_t) {
    let kmsg: *mut ipc_kmsg_full;
    let n: *mut mach_no_senders_notification_t;

    kmsg = ikm_alloc(core::mem::size_of::<mach_no_senders_notification_t>() as u32);
    if kmsg == IKM_NULL {
        printf(
            b"dropped no-senders (0x%p, %u)\n\0".as_ptr() as *const _,
            port,
            mscount,
        );
        ipc_port_release_sonce(port);
        return;
    }

    ikm_init(
        kmsg,
        core::mem::size_of::<mach_no_senders_notification_t>() as u32,
    );
    n = addr_of_mut!((*kmsg).ikm_header) as *mut mach_no_senders_notification_t;
    *n = core::ptr::read(addr_of_mut!(ipc_notify_no_senders_template));

    (*n).not_header.msgh_remote_port = port as u32;
    (*n).not_count = mscount;

    ipc_mqueue_send_always(kmsg);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_send_once(port: *mut ipc_port) {
    let kmsg: *mut ipc_kmsg_full;
    let n: *mut mach_send_once_notification_t;

    kmsg = ikm_alloc(core::mem::size_of::<mach_send_once_notification_t>() as u32);
    if kmsg == IKM_NULL {
        printf(b"dropped send-once (0x%p)\n\0".as_ptr() as *const _, port);
        ipc_port_release_sonce(port);
        return;
    }

    ikm_init(
        kmsg,
        core::mem::size_of::<mach_send_once_notification_t>() as u32,
    );
    n = addr_of_mut!((*kmsg).ikm_header) as *mut mach_send_once_notification_t;
    *n = core::ptr::read(addr_of_mut!(ipc_notify_send_once_template));

    (*n).not_header.msgh_remote_port = port as u32;

    ipc_mqueue_send_always(kmsg);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_notify_dead_name(port: *mut ipc_port, name: mach_port_name_t) {
    let kmsg: *mut ipc_kmsg_full;
    let n: *mut mach_dead_name_notification_t;

    kmsg = ikm_alloc(core::mem::size_of::<mach_dead_name_notification_t>() as u32);
    if kmsg == IKM_NULL {
        printf(
            b"dropped dead-name (0x%p, 0x%x)\n\0".as_ptr() as *const _,
            port,
            name,
        );
        ipc_port_release_sonce(port);
        return;
    }

    ikm_init(
        kmsg,
        core::mem::size_of::<mach_dead_name_notification_t>() as u32,
    );
    n = addr_of_mut!((*kmsg).ikm_header) as *mut mach_dead_name_notification_t;
    *n = core::ptr::read(addr_of_mut!(ipc_notify_dead_name_template));

    (*n).not_header.msgh_remote_port = port as u32;
    (*n).not_port = name;

    ipc_mqueue_send_always(kmsg);
}
