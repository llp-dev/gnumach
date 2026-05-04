//! Port of `ipc/ipc_target.c`.

use crate::ipc_mqueue::ipc_mqueue_init;
use crate::mach_types::{ipc_target, mach_port_name_t};

#[no_mangle]
pub unsafe extern "C" fn ipc_target_init(
    ipt: *mut ipc_target,
    name: mach_port_name_t,
) {
    (*ipt).ipt_name = name;
    ipc_mqueue_init(&mut (*ipt).ipt_messages);
}

#[no_mangle]
pub unsafe extern "C" fn ipc_target_terminate(_ipt: *mut ipc_target) {}
