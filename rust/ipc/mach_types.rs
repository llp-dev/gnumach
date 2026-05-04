//! `#[repr(C)]` mirrors of mach kernel types referenced from Rust.
//!
//! Every C struct that crosses the Rust/C boundary is mirrored field-for-field
//! with the same name and an equivalent type.  Compile-time `size_of` checks
//! are paired with `_Static_assert(offsetof(...) == ...)` in
//! `ipc/ipc_layout_asserts.c` so layout drift in the C side fails the build.
//!
//! Layout assumes the i386 build configuration (NCPUS = 1, MACH_SLOCKS = 0,
//! MACH_HOST = 0, PAE = 1).  Conditional fields (`#if MACH_HOST`,
//! `#if NCPUS > 1`) are NOT mirrored when those conditions are false.

#![allow(dead_code)]

use core::mem::size_of;

// ---------------------------------------------------------------------------
//  Primitive typedefs
// ---------------------------------------------------------------------------

/// `mach/std_types.h`: `typedef unsigned int natural_t;`
pub type natural_t = u32;
/// `mach/std_types.h`: `typedef int integer_t;`
pub type integer_t = i32;
/// `mach/port.h`: `typedef unsigned int mach_port_name_t;`
pub type mach_port_name_t = u32;
/// `mach/port.h`: `typedef vm_offset_t mach_port_t;` — but in kernel,
/// kernel-side mach_port_t is `struct ipc_port *`.  Here we use a 32-bit
/// integer slot since most C uses don't dereference it as a struct pointer.
pub type mach_port_t = u32;
/// `mach/port.h`: `typedef natural_t mach_port_seqno_t;`
pub type mach_port_seqno_t = natural_t;
/// `mach/kern_return.h`: `typedef int kern_return_t;`
pub type kern_return_t = i32;
/// `mach/message.h`: `typedef kern_return_t mach_msg_return_t;`
pub type mach_msg_return_t = kern_return_t;
/// `mach/message.h`: `typedef unsigned int mach_msg_size_t;`
pub type mach_msg_size_t = u32;
/// `mach/message.h`: `typedef integer_t mach_msg_option_t;`
pub type mach_msg_option_t = integer_t;
/// `mach/message.h`: `typedef natural_t mach_msg_timeout_t;`
pub type mach_msg_timeout_t = natural_t;
/// `mach/boolean.h`: `typedef int boolean_t;`
pub type boolean_t = i32;
/// `mach/vm_types.h`: `typedef uintptr_t vm_offset_t;` (i686 → 32-bit)
pub type vm_offset_t = u32;
/// `mach/vm_types.h`: `typedef uintptr_t vm_size_t;`
pub type vm_size_t = u32;
/// `i386/include/mach/i386/vm_param.h`: `#define PAGE_SHIFT 12`
pub const PAGE_SHIFT: u32 = 12;
pub const PAGE_SIZE: vm_size_t = 1 << PAGE_SHIFT;

/// `ipc/ipc_kmsg.h`: per-processor cached kmsg buffer size.
pub const IKM_SAVED_KMSG_SIZE: vm_size_t = PAGE_SIZE;
/// `IKM_SAVED_MSG_SIZE = ikm_less_overhead(IKM_SAVED_KMSG_SIZE)`.
pub const IKM_SAVED_MSG_SIZE: u32 = (PAGE_SIZE - IKM_OVERHEAD_SIZE) as u32;
/// `IKM_OVERHEAD = sizeof(struct ipc_kmsg) - sizeof(mach_msg_header_t) = 16`.
pub const IKM_OVERHEAD_SIZE: vm_size_t = 16;
/// `IKM_EXPAND_FACTOR = (sizeof(mach_port_t)+sizeof(mach_port_name_t)-1) / sizeof(mach_port_name_t)`
/// On i686: (4+4-1)/4 = 1.
pub const IKM_EXPAND_FACTOR: u32 = 1;
/// `kern/sched_prim.h`: `typedef void *event_t;`
pub type event_t = *mut core::ffi::c_void;
/// `kern/sched_prim.h`: `typedef void (*continuation_t)(void);`
pub type continuation_t = Option<unsafe extern "C" fn() -> !>;
/// `kern/mach_clock.h`: `typedef void timer_func_t(void *);`
pub type timer_func_t = Option<unsafe extern "C" fn(*mut core::ffi::c_void)>;

// ---------------------------------------------------------------------------
//  Forward-declared opaque structs
// ---------------------------------------------------------------------------

#[repr(C)] pub struct task         { _opaque: [u8; 0] }
#[repr(C)] pub struct run_queue    { _opaque: [u8; 0] }
#[repr(C)] pub struct pcb          { _opaque: [u8; 0] }
#[repr(C)] pub struct ipc_kmsg     { _opaque: [u8; 0] }
/// Sized opaque mirror for `struct kmem_cache`; internal layout is a black
/// box from Rust.  `_bytes` is `pub` only so callers can construct a
/// zero-initialized static.
#[repr(C, align(4))] pub struct kmem_cache { pub _bytes: [u8; SIZE_OF_KMEM_CACHE] }
#[repr(C)] pub struct vm_map       { _opaque: [u8; 0] }
#[repr(C)] pub struct processor    { _opaque: [u8; 0] }
#[repr(C)] pub struct processor_set{ _opaque: [u8; 0] }
/// `mach/message.h`: `mach_msg_user_header_t` — user-mode view of the
/// message header.  Same first three fields as `mach_msg_header_t`
/// (the type modeled here is the kernel mirror; user-side `mach_port_t`
/// is `mach_port_name_t` = `u32`, which matches `mach_port_t` = pointer
/// = 4 bytes on i686).
#[repr(C, align(4))]
pub struct mach_msg_user_header {
    pub msgh_bits: mach_msg_bits_t,
    pub msgh_size: mach_msg_size_t,
    pub msgh_remote_port: u32,
    pub msgh_local_port_or_payload: u32,
    pub msgh_seqno: mach_port_seqno_t,
    pub msgh_id: mach_msg_id_t,
}
const _: () = assert!(size_of::<mach_msg_user_header>() == 24);

// `struct ipc_space`, `struct ipc_entry`, `struct ipc_port`, `struct ipc_pset`
// are full mirrors defined later in this file (they need other types
// declared below).

pub type task_t           = *mut task;
pub type run_queue_t      = *mut run_queue;
pub type pcb_t            = *mut pcb;
pub type ipc_port_t       = *mut ipc_port;
pub type ipc_space_t      = *mut ipc_space;
pub type ipc_pset_t       = *mut ipc_pset;
pub type vm_map_t         = *mut vm_map;
pub type processor_t      = *mut processor;
pub type processor_set_t  = *mut processor_set;
pub type mach_msg_user_header_t = mach_msg_user_header;

/// `ipc/ipc_port.h`: `typedef unsigned int ipc_port_timestamp_t;`
pub type ipc_port_timestamp_t = u32;

/// `mach/kern_return.h`: success result.
pub const KERN_SUCCESS: kern_return_t = 0;

/// `ipc/ipc_object.h`: object type indices.
pub const IOT_PORT: u32 = 0;
pub const IOT_PORT_SET: u32 = 1;
pub const IOT_NUMBER: usize = 2;

/// `mach/port.h`: `typedef natural_t mach_port_right_t;`
pub type mach_port_right_t = natural_t;
/// `mach/port.h`: `typedef natural_t mach_port_type_t;`
pub type mach_port_type_t = natural_t;
/// `mach/port.h`: `typedef natural_t mach_port_urefs_t;`
pub type mach_port_urefs_t = natural_t;

// ---------------------------------------------------------------------------
//  C struct sizes (pinned by _Static_assert in ipc/ipc_layout_asserts.c).
//  Used at Rust compile time wherever a port needs `sizeof(struct foo)` to
//  pass to a C API such as kmem_cache_init.  Forward-declared types like
//  `struct ipc_space` stay opaque on the Rust side.
// ---------------------------------------------------------------------------

pub const SIZE_OF_VM_MAP:      usize = 84;
pub const SIZE_OF_IPC_SPACE:   usize = 44;
pub const SIZE_OF_IPC_ENTRY:   usize = 16;
pub const SIZE_OF_IPC_PORT:    usize = 80;
pub const SIZE_OF_IPC_PSET:    usize = 20;
pub const SIZE_OF_KMEM_CACHE:  usize = 128;

/// Offset of `messages_sent`/`messages_received` inside `struct task`.
/// Pinned by `_Static_assert` in `ipc/ipc_layout_asserts.c`.
pub const OFFSETOF_TASK_MESSAGES_SENT:     usize = 160;
pub const OFFSETOF_TASK_MESSAGES_RECEIVED: usize = 164;
/// Offset of `active_thread` inside `struct percpu`.
pub const OFFSETOF_PERCPU_ACTIVE_THREAD: usize = 600;

/// Offsets of `map`, `itk_space`, and `name` inside `struct task`.
pub const OFFSETOF_TASK_MAP:       usize = 8;
pub const OFFSETOF_TASK_ITK_SPACE: usize = 128;
pub const OFFSETOF_TASK_NAME:      usize = 168;
pub const TASK_NAME_SIZE:          usize = 32;

// ---------------------------------------------------------------------------
//  Lock primitives
// ---------------------------------------------------------------------------

/// `kern/lock.h`: with `MACH_SLOCKS == 0`, `decl_simple_lock_data` expands
/// to `struct simple_lock_data_empty { struct {} is_a_simple_lock; }` which
/// gcc treats as a zero-sized struct.
#[repr(C)]
pub struct simple_lock_data_t {}

const _: () = assert!(size_of::<simple_lock_data_t>() == 0);

// ---------------------------------------------------------------------------
//  Common embedded structs
// ---------------------------------------------------------------------------

/// `kern/queue.h`: `typedef struct queue_entry queue_chain_t;`
#[repr(C)]
pub struct queue_chain_t {
    pub next: *mut queue_entry,
    pub prev: *mut queue_entry,
}

#[repr(C)] pub struct queue_entry  { _opaque: [u8; 0] }

const _: () = assert!(size_of::<queue_chain_t>() == 8);

/// `kern/timer.h`: `struct timer { unsigned low_bits, high_bits, high_bits_check, tstamp; };`
#[repr(C)]
pub struct timer_data_t {
    pub low_bits: u32,
    pub high_bits: u32,
    pub high_bits_check: u32,
    pub tstamp: u32,
}

const _: () = assert!(size_of::<timer_data_t>() == 16);

/// `kern/timer.h`: `struct timer_save { unsigned low, high; };`
#[repr(C)]
pub struct timer_save_data_t {
    pub low: u32,
    pub high: u32,
}

const _: () = assert!(size_of::<timer_save_data_t>() == 8);

/// `mach/time_value.h`: `struct time_value64 { int64_t seconds; int64_t nanoseconds; };`
/// On i686 SysV ABI `int64_t` has alignment 4, so this whole struct is 4-aligned.
#[repr(C)]
pub struct time_value64_t {
    pub seconds: i64,
    pub nanoseconds: i64,
}

const _: () = assert!(size_of::<time_value64_t>() == 16);

/// `kern/mach_clock.h`: `struct timeout`.
#[repr(C)]
pub struct timeout_data_t {
    pub chain: queue_chain_t,
    pub fcn: timer_func_t,
    pub param: *mut core::ffi::c_void,
    pub t_time: u32, /* unsigned long on i686 = 4 bytes */
    pub set: u8,
    pub _pad: [u8; 3],
}

const _: () = assert!(size_of::<timeout_data_t>() == 24);

// ---------------------------------------------------------------------------
//  IPC core types
// ---------------------------------------------------------------------------

/// `ipc/ipc_object.h`: `typedef unsigned int ipc_object_refs_t;`
pub type ipc_object_refs_t = u32;
/// `ipc/ipc_object.h`: `typedef unsigned int ipc_object_bits_t;`
pub type ipc_object_bits_t = u32;

#[repr(C)]
pub struct ipc_object {
    pub io_lock_data: simple_lock_data_t,
    pub io_references: ipc_object_refs_t,
    pub io_bits: ipc_object_bits_t,
}

const _: () = assert!(size_of::<ipc_object>() == 8);

/// `ipc/ipc_kmsg_queue.h`: `struct ipc_kmsg_queue { struct ipc_kmsg *ikmq_base; };`
#[repr(C)]
pub struct ipc_kmsg_queue {
    pub ikmq_base: *mut ipc_kmsg,
}

const _: () = assert!(size_of::<ipc_kmsg_queue>() == 4);

/// `ipc/ipc_thread.h`: `typedef thread_t ipc_thread_t;`
pub type ipc_thread_t = *mut thread;

/// `kern/thread.h`: `#define THREAD_NULL ((thread_t)0)`
pub const ITH_NULL: ipc_thread_t = core::ptr::null_mut();

/// `ipc/ipc_thread.h`: `struct ipc_thread_queue { ipc_thread_t ithq_base; };`
#[repr(C)]
pub struct ipc_thread_queue {
    pub ithq_base: ipc_thread_t,
}

const _: () = assert!(size_of::<ipc_thread_queue>() == 4);

/// `ipc/ipc_mqueue.h`
#[repr(C)]
pub struct ipc_mqueue {
    pub imq_lock_data: simple_lock_data_t,
    pub imq_messages: ipc_kmsg_queue,
    pub imq_threads: ipc_thread_queue,
}

const _: () = assert!(size_of::<ipc_mqueue>() == 8);

/// `ipc/ipc_target.h`
#[repr(C)]
pub struct ipc_target {
    pub ipt_object: ipc_object,
    pub ipt_name: mach_port_name_t,
    pub ipt_messages: ipc_mqueue,
}

const _: () = assert!(size_of::<ipc_target>() == 20);

/// `ipc/ipc_table.h`: `typedef unsigned int ipc_table_index_t;`
pub type ipc_table_index_t = u32;
/// `ipc/ipc_table.h`: `typedef unsigned int ipc_table_elems_t;`
pub type ipc_table_elems_t = u32;

#[repr(C)]
pub struct ipc_table_size {
    pub its_size: ipc_table_elems_t,
}

const _: () = assert!(size_of::<ipc_table_size>() == 4);

/// `ipc/ipc_table.h`: `typedef struct ipc_table_size *ipc_table_size_t;`
pub type ipc_table_size_t = *mut ipc_table_size;
/// `ipc/ipc_table.h`: `#define ITS_NULL ((ipc_table_size_t) 0)`
pub const ITS_NULL: ipc_table_size_t = core::ptr::null_mut();

/// `ipc/ipc_port.h`: `typedef ipc_table_index_t ipc_port_request_index_t;`
pub type ipc_port_request_index_t = ipc_table_index_t;

/// `ipc/ipc_port.h`: union `notify` field of `ipc_port_request`.
#[repr(C)]
pub union ipc_port_request_notify {
    pub port: *mut ipc_port,
    pub index: ipc_port_request_index_t,
}
const _: () = assert!(size_of::<ipc_port_request_notify>() == 4);

/// `ipc/ipc_port.h`: union `name` field of `ipc_port_request`.
#[repr(C)]
pub union ipc_port_request_name {
    pub name: mach_port_name_t,
    pub size: *mut ipc_table_size,
}
const _: () = assert!(size_of::<ipc_port_request_name>() == 4);

/// `ipc/ipc_port.h`: `struct ipc_port_request`.
#[repr(C)]
pub struct ipc_port_request {
    pub notify: ipc_port_request_notify,
    pub name: ipc_port_request_name,
}

const _: () = assert!(size_of::<ipc_port_request>() == 8);

// ---------------------------------------------------------------------------
//  mach/message.h — message header / type-descriptor types and constants
// ---------------------------------------------------------------------------

/// `mach/message.h`: `typedef unsigned int mach_msg_bits_t;`
pub type mach_msg_bits_t = u32;
pub type mach_msg_seqno_t = natural_t;
pub type mach_msg_id_t = integer_t;
pub type mach_msg_type_name_t = u32;
pub type mach_msg_type_size_t = u32;

/// `mach/port.h`: `typedef unsigned int mach_port_mscount_t;`
pub type mach_port_mscount_t = u32;
/// `i386/include/mach/i386/vm_types.h`: `typedef unsigned long long_natural_t;`
pub type long_natural_t = u32;

/// `mach/port.h`: `typedef unsigned int mach_port_msgcount_t;`
pub type mach_port_msgcount_t = u32;
/// `mach/port.h`: `typedef unsigned int mach_port_rights_t;`
pub type mach_port_rights_t = u32;

/// `ipc/ipc_port.h`: `typedef struct ipc_port_request *ipc_port_request_t;`
pub type ipc_port_request_t = *mut ipc_port_request;

pub const MACH_MSGH_BITS_COMPLEX: u32 = 0x80000000;

/// `MACH_MSGH_BITS(remote, local)`.
#[allow(non_snake_case)]
pub const fn MACH_MSGH_BITS(remote: u32, local: u32) -> u32 {
    remote | (local << 8)
}

pub const MACH_MSG_TYPE_INTEGER_32:     u32 = 2;
pub const MACH_MSG_TYPE_PORT_NAME:      u32 = 15;
pub const MACH_MSG_TYPE_MOVE_RECEIVE:   u32 = 16;
pub const MACH_MSG_TYPE_MOVE_SEND_ONCE: u32 = 18;
pub const MACH_MSG_TYPE_PORT_RECEIVE:   u32 = MACH_MSG_TYPE_MOVE_RECEIVE;
pub const MACH_MSG_TYPE_PORT_SEND_ONCE: u32 = MACH_MSG_TYPE_MOVE_SEND_ONCE;

pub const MACH_PORT_NULL: u32 = 0;
pub const MACH_MSG_TIMEOUT_NONE: mach_msg_timeout_t = 0;
pub const MACH_MSG_OPTION_NONE:  mach_msg_option_t = 0;
pub const MACH_MSG_SIZE_MAX:     mach_msg_size_t = !0;

/// `mach/message.h`: option-bit constants used by mach_msg.
pub const MACH_SEND_MSG:     mach_msg_option_t = 0x0000_0001;
pub const MACH_RCV_MSG:      mach_msg_option_t = 0x0000_0002;
pub const MACH_SEND_CANCEL:  mach_msg_option_t = 0x0000_0008;
pub const MACH_RCV_NOTIFY:   mach_msg_option_t = 0x0000_0200;
pub const MACH_RCV_LARGE:    mach_msg_option_t = 0x0000_0800;
pub const MACH_SEND_NOTIFY:  mach_msg_option_t = 0x0000_0020;

pub const MACH_SEND_WILL_NOTIFY: mach_msg_return_t = 0x1000_0005;

pub const MACH_MSG_MASK: mach_msg_return_t = 0x0000_3c00;
pub const MACH_SEND_ALWAYS: mach_msg_option_t = 0x00010000;

/// MACH_NOTIFY_FIRST + offsets (notify.h, octal).
pub const MACH_NOTIFY_FIRST:          mach_msg_id_t = 0o100;
pub const MACH_NOTIFY_PORT_DELETED:   mach_msg_id_t = MACH_NOTIFY_FIRST + 0o001;
pub const MACH_NOTIFY_MSG_ACCEPTED:   mach_msg_id_t = MACH_NOTIFY_FIRST + 0o002;
pub const MACH_NOTIFY_PORT_DESTROYED: mach_msg_id_t = MACH_NOTIFY_FIRST + 0o005;
pub const MACH_NOTIFY_NO_SENDERS:     mach_msg_id_t = MACH_NOTIFY_FIRST + 0o006;
pub const MACH_NOTIFY_SEND_ONCE:      mach_msg_id_t = MACH_NOTIFY_FIRST + 0o007;
pub const MACH_NOTIFY_DEAD_NAME:      mach_msg_id_t = MACH_NOTIFY_FIRST + 0o010;

/// `mach/message.h`: `mach_msg_header_t`.
///
/// The two C unions (`msgh_remote_port` / `msgh_remote_port_do_not_use` and
/// likewise for local) collapse to a single `u32` here because both arms are
/// 4 bytes in this build.  Memory layout is identical; the union exists in C
/// only for typing.
#[repr(C)]
pub struct mach_msg_header_t {
    pub msgh_bits: mach_msg_bits_t,
    pub msgh_size: mach_msg_size_t,
    pub msgh_remote_port: u32,
    pub msgh_local_port: u32,
    pub msgh_seqno: mach_port_seqno_t,
    pub msgh_id: mach_msg_id_t,
}
const _: () = assert!(size_of::<mach_msg_header_t>() == 24);

/// `mach/message.h`:
/// ```text
/// typedef struct {
///     unsigned int msgt_name : 8,
///                  msgt_size : 8,
///                  msgt_number : 12,
///                  msgt_inline : 1,
///                  msgt_longform : 1,
///                  msgt_deallocate : 1,
///                  msgt_unused : 1;
/// } __attribute__((aligned(__alignof__(uintptr_t)))) mach_msg_type_t;
/// ```
///
/// gcc/i686 lays out bitfields LSB-first.  Modelled here as a single `u32`
/// plus a const-fn packer; field-by-field assignment in the C originals is
/// replaced by one packed write per template.
#[repr(C, align(4))]
pub struct mach_msg_type_t {
    pub bits: u32,
}
const _: () = assert!(size_of::<mach_msg_type_t>() == 4);

impl mach_msg_type_t {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        name: u32,
        size: u32,
        number: u32,
        inline_: u32,
        longform: u32,
        deallocate: u32,
        unused: u32,
    ) -> Self {
        Self {
            bits: (name & 0xff)
                | ((size & 0xff) << 8)
                | ((number & 0x0fff) << 16)
                | ((inline_ & 1) << 28)
                | ((longform & 1) << 29)
                | ((deallocate & 1) << 30)
                | ((unused & 1) << 31),
        }
    }

    /// `msgt_name : 8` (bits 0..8)
    #[inline]
    pub const fn msgt_name(&self) -> u32 { self.bits & 0xff }
    /// `msgt_size : 8` (bits 8..16)
    #[inline]
    pub const fn msgt_size(&self) -> u32 { (self.bits >> 8) & 0xff }
    /// `msgt_number : 12` (bits 16..28)
    #[inline]
    pub const fn msgt_number(&self) -> u32 { (self.bits >> 16) & 0x0fff }
    /// `msgt_inline : 1` (bit 28)
    #[inline]
    pub const fn msgt_inline(&self) -> u32 { (self.bits >> 28) & 1 }
    /// `msgt_longform : 1` (bit 29)
    #[inline]
    pub const fn msgt_longform(&self) -> u32 { (self.bits >> 29) & 1 }
    /// `msgt_deallocate : 1` (bit 30)
    #[inline]
    pub const fn msgt_deallocate(&self) -> u32 { (self.bits >> 30) & 1 }
    /// `msgt_unused : 1` (bit 31)
    #[inline]
    pub const fn msgt_unused(&self) -> u32 { (self.bits >> 31) & 1 }

    /// Setters; preserve other fields.
    #[inline]
    pub fn set_msgt_name(&mut self, v: u32) {
        self.bits = (self.bits & !0xff) | (v & 0xff);
    }
    #[inline]
    pub fn set_msgt_size(&mut self, v: u32) {
        self.bits = (self.bits & !(0xff << 8)) | ((v & 0xff) << 8);
    }
    #[inline]
    pub fn set_msgt_deallocate(&mut self, v: u32) {
        self.bits = (self.bits & !(1 << 30)) | ((v & 1) << 30);
    }
}

/// `mach/message.h`: `mach_msg_type_long_t` — extended descriptor.
#[repr(C, align(4))]
pub struct mach_msg_type_long_t {
    pub msgtl_header: mach_msg_type_t,
    pub msgtl_name:   u16,
    pub msgtl_size:   u16,
    pub msgtl_number: natural_t,
}
const _: () = assert!(size_of::<mach_msg_type_long_t>() == 12);

// ---------------------------------------------------------------------------
//  mach/notify.h — notification message structs
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct mach_port_deleted_notification_t {
    pub not_header: mach_msg_header_t,
    pub not_type:   mach_msg_type_t,
    pub not_port:   mach_port_name_t,
}
const _: () = assert!(size_of::<mach_port_deleted_notification_t>() == 32);

#[repr(C)]
pub struct mach_msg_accepted_notification_t {
    pub not_header: mach_msg_header_t,
    pub not_type:   mach_msg_type_t,
    pub not_port:   mach_port_name_t,
}
const _: () = assert!(size_of::<mach_msg_accepted_notification_t>() == 32);

#[repr(C)]
pub struct mach_port_destroyed_notification_t {
    pub not_header: mach_msg_header_t,
    pub not_type:   mach_msg_type_t,
    pub not_port:   u32,
}
const _: () = assert!(size_of::<mach_port_destroyed_notification_t>() == 32);

#[repr(C)]
pub struct mach_no_senders_notification_t {
    pub not_header: mach_msg_header_t,
    pub not_type:   mach_msg_type_t,
    pub not_count:  u32,
}
const _: () = assert!(size_of::<mach_no_senders_notification_t>() == 32);

#[repr(C)]
pub struct mach_send_once_notification_t {
    pub not_header: mach_msg_header_t,
}
const _: () = assert!(size_of::<mach_send_once_notification_t>() == 24);

#[repr(C)]
pub struct mach_dead_name_notification_t {
    pub not_header: mach_msg_header_t,
    pub not_type:   mach_msg_type_t,
    pub not_port:   mach_port_name_t,
}
const _: () = assert!(size_of::<mach_dead_name_notification_t>() == 32);

// ---------------------------------------------------------------------------
//  ipc/ipc_kmsg.h — kernel message buffer (subset used by ports)
// ---------------------------------------------------------------------------

// `struct ipc_marequest` full mirror is `ipc_marequest_full`, declared
// further below; here we just expose the typedef and null constant.
pub type ipc_marequest_t = *mut ipc_marequest_full;
pub const IMAR_NULL: ipc_marequest_t = core::ptr::null_mut();

/// `struct ipc_kmsg` — header plus bookkeeping prefix.  Renamed
/// `ipc_kmsg_full` to disambiguate from the opaque forward declaration of
/// `ipc_kmsg` used elsewhere.
#[repr(C)]
pub struct ipc_kmsg_full {
    pub ikm_next: *mut ipc_kmsg_full,
    pub ikm_prev: *mut ipc_kmsg_full,
    pub ikm_size: vm_size_t,
    pub ikm_marequest: ipc_marequest_t,
    pub ikm_header: mach_msg_header_t,
}
const _: () = assert!(size_of::<ipc_kmsg_full>() == 40);

pub const IKM_NULL: *mut ipc_kmsg_full = core::ptr::null_mut();

/// `#define IKM_OVERHEAD (sizeof(struct ipc_kmsg) - sizeof(mach_msg_header_t))`
pub const IKM_OVERHEAD: vm_size_t =
    (size_of::<ipc_kmsg_full>() - size_of::<mach_msg_header_t>()) as vm_size_t;

/// `PORT_T_SIZE_IN_BITS = sizeof(mach_port_t) * 8`.
pub const PORT_T_SIZE_IN_BITS:      u32 = 32;
/// `PORT_NAME_T_SIZE_IN_BITS = sizeof(mach_port_name_t) * 8`.
pub const PORT_NAME_T_SIZE_IN_BITS: u32 = 32;

// ---------------------------------------------------------------------------
//  struct thread (`kern/thread.h`)
//
//  Mirrored field-for-field.  Conditional members (#if MACH_HOST, #if NCPUS>1)
//  are present only when those conditions are true at C compile time.
// ---------------------------------------------------------------------------

/// `union { struct { unsigned state:16, wake_active:1, active:1; }; event_t event_key; }`.
/// All variants are 4 bytes wide; we expose both representations so callers
/// can pick.  Bitfield manipulation stays in C.
#[repr(C)]
pub union thread_state_or_event_key {
    pub state_bits: u32,
    pub event_key: event_t,
}

const _: () = assert!(size_of::<thread_state_or_event_key>() == 4);

/// `union { mach_msg_size_t msize; struct ipc_kmsg *kmsg; } data;`
#[repr(C)]
pub union thread_data {
    pub msize: mach_msg_size_t,
    pub kmsg: *mut ipc_kmsg,
}

const _: () = assert!(size_of::<thread_data>() == 4);

/// State saved when thread's stack is discarded — the receive variant.
#[repr(C)]
pub struct thread_saved_receive {
    pub msg: *mut mach_msg_user_header_t,
    pub option: mach_msg_option_t,
    pub rcv_size: mach_msg_size_t,
    pub timeout: mach_msg_timeout_t,
    pub notify: mach_port_name_t,
    pub object: *mut ipc_object,
    pub mqueue: *mut ipc_mqueue,
}

const _: () = assert!(size_of::<thread_saved_receive>() == 28);

/// `union { struct {receive}; struct {exception}; void *other; } saved;`
/// Largest variant is `receive` at 28 bytes.
#[repr(C)]
pub union thread_saved {
    pub receive: core::mem::ManuallyDrop<thread_saved_receive>,
    /* The exception variant (16 bytes) and `other` (4 bytes) fit inside the
       28-byte receive variant — no need to model them explicitly here. */
}

const _: () = assert!(size_of::<thread_saved>() == 28);

/// `kern/thread.h`: `struct thread`.  Full field-by-field mirror.
#[repr(C)]
pub struct thread {
    /* Run queues */
    pub links: queue_chain_t,
    pub runq: run_queue_t,

    /* Task information */
    pub task: task_t,
    pub thread_list: queue_chain_t,

    /* Flags */
    pub state_or_event_key: thread_state_or_event_key,

    /* Thread bookkeeping */
    pub pset_threads: queue_chain_t,

    /* Self-preservation */
    pub lock: simple_lock_data_t,
    pub ref_count: i32,

    /* Hardware state */
    pub pcb: pcb_t,
    pub kernel_stack: vm_offset_t,
    pub stack_privilege: vm_offset_t,

    /* Swapping information */
    pub swap_func: continuation_t,

    /* Blocking information */
    pub wait_event: event_t,
    pub suspend_count: i32,
    pub wait_result: kern_return_t,

    /* Scheduling information */
    pub priority: i32,
    pub max_priority: i32,
    pub sched_pri: i32,
    pub sched_data: i32,
    pub policy: i32,
    pub depress_priority: i32,
    pub cpu_usage: u32,
    pub sched_usage: u32,
    pub sched_stamp: u32,

    /* VM global variables */
    pub recover: vm_offset_t,
    pub vm_privilege: u32,

    /* User-visible scheduling state */
    pub user_stop_count: i32,

    /* IPC data structures */
    pub ith_next: *mut thread,
    pub ith_prev: *mut thread,
    pub ith_state: mach_msg_return_t,
    pub data: thread_data,
    pub ith_seqno: mach_port_seqno_t,
    pub ith_messages: ipc_kmsg_queue,
    pub ith_lock_data: simple_lock_data_t,
    pub ith_self: *mut ipc_port,
    pub ith_sself: *mut ipc_port,
    pub ith_exception: *mut ipc_port,
    pub ith_mig_reply: mach_port_name_t,
    pub ith_rpc_reply: *mut ipc_port,

    /* State saved when thread's stack is discarded */
    pub saved: thread_saved,

    /* Timing data structures */
    pub user_timer: timer_data_t,
    pub system_timer: timer_data_t,
    pub user_timer_save: timer_save_data_t,
    pub system_timer_save: timer_save_data_t,
    pub cpu_delta: u32,
    pub sched_delta: u32,

    /* Creation time stamp */
    pub creation_time: time_value64_t,

    /* Time-outs */
    pub timer: timeout_data_t,
    pub depress_timer: timeout_data_t,

    /* Ast/Halt data structures */
    pub ast: i32,

    /* Processor data structures */
    pub processor_set: processor_set_t,
    pub bound_processor: processor_t,

    /* THREAD_NAME_SIZE = TASK_NAME_SIZE = 32 */
    pub name: [core::ffi::c_char; 32],
}

const _: () = assert!(size_of::<thread>() == 352);

// ---------------------------------------------------------------------------
//  kern/lock.h — read/write `struct lock` (8 bytes; treated as opaque blob)
// ---------------------------------------------------------------------------

/// `kern/lock.h`: `struct lock`.  Internal layout is a black box from Rust;
/// callers go through `lock_write` / `lock_done` C functions.
#[repr(C, align(4))]
pub struct lock_data_t { _bytes: [u8; 8] }
const _: () = assert!(size_of::<lock_data_t>() == 8);

pub type lock_t = *mut lock_data_t;

// ---------------------------------------------------------------------------
//  kern/rdxtree.h — radix tree (`struct rdxtree` is 8 bytes; opaque)
// ---------------------------------------------------------------------------

#[repr(C, align(4))]
pub struct rdxtree { pub _bytes: [u8; 8] }

/// `kern/rdxtree_i.h`: `struct rdxtree_iter { void *node; rdxtree_key_t key; };`
#[repr(C)]
pub struct rdxtree_iter {
    pub node: *mut core::ffi::c_void,
    pub key: rdxtree_key_t,
}
const _: () = assert!(size_of::<rdxtree_iter>() == 8);
const _: () = assert!(size_of::<rdxtree>() == 8);

pub type rdxtree_key_t = u32; /* RDXTREE_KEY_32 is set in CFLAGS */

// ---------------------------------------------------------------------------
//  ipc/ipc_entry.h — `struct ipc_entry`
// ---------------------------------------------------------------------------

pub type ipc_entry_bits_t = u32;

/// `union { struct ipc_entry *next_free; unsigned int request; } index;`
#[repr(C)]
pub union ipc_entry_index_u {
    pub next_free: *mut ipc_entry,
    pub request: u32,
}
const _: () = assert!(size_of::<ipc_entry_index_u>() == 4);

#[repr(C)]
pub struct ipc_entry {
    pub ie_name: mach_port_name_t,
    pub ie_bits: ipc_entry_bits_t,
    pub ie_object: *mut ipc_object,
    pub index: ipc_entry_index_u,
}
const _: () = assert!(size_of::<ipc_entry>() == 16);

pub type ipc_entry_t = *mut ipc_entry;
pub const IE_NULL: ipc_entry_t = core::ptr::null_mut();

// IE_BITS_*
pub const IE_BITS_UREFS_MASK: ipc_entry_bits_t = 0x0000_ffff;
pub const IE_BITS_TYPE_MASK:  ipc_entry_bits_t = 0x001f_0000;
pub const IE_BITS_MAREQUEST:  ipc_entry_bits_t = 0x0020_0000;

// ---------------------------------------------------------------------------
//  ipc/ipc_space.h — `struct ipc_space`
// ---------------------------------------------------------------------------

pub type ipc_space_refs_t = u32;
pub type size_t = usize; /* size_t is 4 bytes on i686 */

#[repr(C)]
pub struct ipc_space {
    pub is_ref_lock_data: simple_lock_data_t, /* 0 bytes */
    pub is_references: ipc_space_refs_t,
    pub is_lock_data: lock_data_t,
    pub is_active: boolean_t,
    pub is_map: rdxtree,
    pub is_size: size_t,
    pub is_reverse_map: rdxtree,
    pub is_free_list: ipc_entry_t,
    pub is_free_list_size: size_t,
}
const _: () = assert!(size_of::<ipc_space>() == 44);

pub const IS_NULL: ipc_space_t = core::ptr::null_mut();

/// `IO_NULL = NULL` for `ipc_object_t`.
pub const IO_NULL: *mut ipc_object = core::ptr::null_mut();

/// `IP_NULL = (ipc_port_t) IO_NULL = NULL`.
pub const IP_NULL: ipc_port_t = core::ptr::null_mut();

// ---------------------------------------------------------------------------
//  ipc/ipc_marequest.h — msg-accepted-request bookkeeping
// ---------------------------------------------------------------------------

/// Full mirror of `struct ipc_marequest` (matches the existing
/// `ipc_marequest` opaque forward decl declared earlier; we re-declare with
/// the full layout here).
#[repr(C)]
pub struct ipc_marequest_full {
    pub imar_space: *mut ipc_space,
    pub imar_name: mach_port_name_t,
    pub imar_soright: *mut ipc_port,
    pub imar_next: *mut ipc_marequest_full,
}
const _: () = assert!(size_of::<ipc_marequest_full>() == 16);

/// `IPC_MAREQUEST_SIZE` from `ipc/ipc_marequest.h`.
pub const IPC_MAREQUEST_SIZE: u32 = 16;

/// File-static struct from `ipc/ipc_marequest.c`.
#[repr(C)]
pub struct ipc_marequest_bucket {
    pub imarb_lock_data: simple_lock_data_t, /* 0 bytes */
    pub imarb_head: *mut ipc_marequest_full,
}
const _: () = assert!(size_of::<ipc_marequest_bucket>() == 4);

pub type ipc_marequest_bucket_t = *mut ipc_marequest_bucket;
pub const IMARB_NULL: ipc_marequest_bucket_t = core::ptr::null_mut();

// ---------------------------------------------------------------------------
//  mach/port.h / message.h — additional constants
// ---------------------------------------------------------------------------

/// `MACH_PORT_TYPE(right)` packs a right number into a port-type bit.
#[allow(non_snake_case)]
pub const fn MACH_PORT_TYPE(right: u32) -> u32 {
    1u32 << (right + 16)
}

pub const MACH_PORT_RIGHT_SEND:      u32 = 0;
pub const MACH_PORT_RIGHT_RECEIVE:   u32 = 1;
pub const MACH_PORT_RIGHT_SEND_ONCE: u32 = 2;

pub const MACH_PORT_TYPE_SEND: u32 = MACH_PORT_TYPE(MACH_PORT_RIGHT_SEND);
pub const MACH_PORT_TYPE_RECEIVE: u32 = MACH_PORT_TYPE(MACH_PORT_RIGHT_RECEIVE);
pub const MACH_PORT_TYPE_SEND_ONCE: u32 = MACH_PORT_TYPE(MACH_PORT_RIGHT_SEND_ONCE);
pub const MACH_PORT_TYPE_SEND_RECEIVE: u32 =
    MACH_PORT_TYPE_SEND | MACH_PORT_TYPE_RECEIVE;

pub const MACH_PORT_NAME_NULL: mach_port_name_t = 0;
pub const MACH_PORT_NAME_DEAD: mach_port_name_t = !0;

/// `mach/kern_return.h`: error codes used by ipc_entry.
pub const KERN_NO_SPACE:           kern_return_t = 3;
pub const KERN_INVALID_ARGUMENT:   kern_return_t = 4;
pub const KERN_FAILURE:            kern_return_t = 5;
pub const KERN_RESOURCE_SHORTAGE:  kern_return_t = 6;
pub const KERN_NAME_EXISTS:        kern_return_t = 13;
pub const KERN_INVALID_NAME:       kern_return_t = 15;
pub const KERN_INVALID_TASK:       kern_return_t = 16;
pub const KERN_INVALID_RIGHT:      kern_return_t = 17;
pub const KERN_INVALID_VALUE:      kern_return_t = 18;
pub const KERN_UREFS_OVERFLOW:     kern_return_t = 19;
pub const KERN_INVALID_CAPABILITY: kern_return_t = 20;
pub const KERN_RIGHT_EXISTS:       kern_return_t = 21;

/// `mach/port.h`: rights/types used by ipc_right.
pub const MACH_PORT_RIGHT_NUMBER: u32 = 5;
pub const MACH_PORT_TYPE_DNREQUEST:  u32 = 0x8000_0000;
pub const MACH_PORT_TYPE_MAREQUEST:  u32 = 0x4000_0000;
pub const MACH_PORT_TYPE_SEND_RIGHTS: u32 =
    MACH_PORT_TYPE_SEND | MACH_PORT_TYPE_SEND_ONCE;

/// `ipc/ipc_entry.h`: `IE_BITS_RIGHT_MASK = 0x003fffff` (urefs+type+marequest).
pub const IE_BITS_RIGHT_MASK: u32 = 0x003f_ffff;

/// `ipc/port.h`: `mach_port_delta_t` is `integer_t`.
pub type mach_port_delta_t = i32;

/// `i386/i386/vm_param.h`: kernel virtual base.  Used by the rdxtree key
/// derivation in `ipc/ipc_space.h::KEY`.
pub const VM_MIN_KERNEL_ADDRESS: u32 = 0xC000_0000;

/// `IO_DEAD = ((ipc_object_t) -1)`.
pub const IO_DEAD: *mut ipc_object = (!0usize) as *mut ipc_object;

/// `kern/host.h`: `typedef struct host *host_t;`  Opaque to Rust.
#[repr(C)] pub struct host { _opaque: [u8; 0] }
pub type host_t = *mut host;
/// `kern/host.h`: `HOST_NULL`.
pub const HOST_NULL: host_t = core::ptr::null_mut();

/// `mach/kern_return.h`: `KERN_INVALID_HOST = 22`.
pub const KERN_INVALID_HOST: kern_return_t = 22;

/// `vm/vm_map.h`: `typedef struct vm_map_copy *vm_map_copy_t;`  Opaque.
#[repr(C)] pub struct vm_map_copy { _opaque: [u8; 0] }
pub type vm_map_copy_t = *mut vm_map_copy;

/// `round_page(x) = (x + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1)`.
#[inline]
pub const fn round_page(x: vm_size_t) -> vm_size_t {
    (x + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// `mach/port.h`: `mach_port_status_t`.
#[repr(C)]
pub struct mach_port_status_t {
    pub mps_pset:     mach_port_name_t,
    pub mps_seqno:    mach_port_seqno_t,
    pub mps_mscount:  mach_port_mscount_t,
    pub mps_qlimit:   mach_port_msgcount_t,
    pub mps_msgcount: mach_port_msgcount_t,
    pub mps_sorights: mach_port_rights_t,
    pub mps_srights:  boolean_t,
    pub mps_pdrequest: boolean_t,
    pub mps_nsrequest: boolean_t,
}
const _: () = assert!(size_of::<mach_port_status_t>() == 36);

/// `mach/port.h`: `MACH_PORT_QLIMIT_MAX = 16`.
pub const MACH_PORT_QLIMIT_MAX:    mach_port_msgcount_t = 16;
pub const MACH_PORT_KTYPE_NONE:        u32 = 0;
pub const MACH_PORT_KTYPE_USER_DEVICE: u32 = 28;
pub type mach_port_ktype_t = u32;

/// `mach/vm_prot.h`: VM protection bits.
pub type vm_prot_t = u32;
pub const VM_PROT_NONE:  vm_prot_t = 0;
pub const VM_PROT_READ:  vm_prot_t = 0x01;
pub const VM_PROT_WRITE: vm_prot_t = 0x02;

/// `vm/vm_map.h`: `VM_MAP_COPY_NULL = NULL`.
pub const VM_MAP_COPY_NULL: vm_map_copy_t = core::ptr::null_mut();

/// `kern/ipc_kobject.h`: `IKO_NULL = 0`, `IKOT_USER_DEVICE = 28`.
pub const IKO_NULL:           ipc_kobject_t = 0;
pub const IKOT_USER_DEVICE:   u32 = 28;
pub const IKOT_PAGING_REQUEST: u32 = 9;
pub const IKOT_DEVICE:         u32 = 10;

/// `MACH_MSG_TYPE_PORT_ANY_RIGHT(x)` — port-types for *port rights* (move/make).
#[inline]
#[allow(non_snake_case)]
pub const fn MACH_MSG_TYPE_PORT_ANY_RIGHT(x: u32) -> bool {
    x >= MACH_MSG_TYPE_MOVE_RECEIVE && x <= MACH_MSG_TYPE_MOVE_SEND_ONCE
}

/// `mach/message.h`: `typedef natural_t mach_msg_type_number_t;`
pub type mach_msg_type_number_t = natural_t;

/// `ipc/ipc_table.h`: `typedef ipc_table_elems_t ipc_entry_num_t;`
pub type ipc_entry_num_t = ipc_table_elems_t;

/// `ipc/port.h`: `MACH_PORT_UREFS_MAX = (1 << 16) - 1`.
pub const MACH_PORT_UREFS_MAX: u32 = (1 << 16) - 1;

/// `mach/port.h`: `MACH_PORT_TYPE_DEAD_NAME = MACH_PORT_TYPE(MACH_PORT_RIGHT_DEAD_NAME=4)`.
pub const MACH_PORT_RIGHT_DEAD_NAME: u32 = 4;
pub const MACH_PORT_TYPE_DEAD_NAME:  u32 = MACH_PORT_TYPE(MACH_PORT_RIGHT_DEAD_NAME);
pub const MACH_PORT_TYPE_NONE:       u32 = 0;

/// `mach/port.h`: combinations.
pub const MACH_PORT_TYPE_PORT_RIGHTS: u32 =
    MACH_PORT_TYPE_SEND | MACH_PORT_TYPE_RECEIVE | MACH_PORT_TYPE_SEND_ONCE;
pub const MACH_PORT_TYPE_PORT_OR_DEAD: u32 =
    MACH_PORT_TYPE_PORT_RIGHTS | MACH_PORT_TYPE_DEAD_NAME;
pub const MACH_PORT_TYPE_ALL_RIGHTS: u32 =
    MACH_PORT_TYPE_PORT_OR_DEAD | MACH_PORT_TYPE_PORT_SET;

/// `mach/message.h`: type-name codes.
pub const MACH_MSG_TYPE_MOVE_SEND:      u32 = 17;
pub const MACH_MSG_TYPE_COPY_SEND:      u32 = 19;
pub const MACH_MSG_TYPE_MAKE_SEND:      u32 = 20;
pub const MACH_MSG_TYPE_MAKE_SEND_ONCE: u32 = 21;
pub const MACH_MSG_TYPE_PORT_SEND:      u32 = MACH_MSG_TYPE_MOVE_SEND;

/// `ipc/ipc_object.h`: `IO_BITS_PROTECTED_PAYLOAD = 0x40000000`.
pub const IO_BITS_PROTECTED_PAYLOAD: ipc_object_bits_t = 0x4000_0000;

/// `ipc/ipc_space.h`: `IS_FREE_LIST_SIZE_LIMIT = 64`.
pub const IS_FREE_LIST_SIZE_LIMIT: usize = 64;

// mach/message.h return codes (subset).
pub const MACH_MSG_SUCCESS:                mach_msg_return_t = 0x0000_0000;
pub const MACH_SEND_NOTIFY_IN_PROGRESS:    mach_msg_return_t = 0x1000_0006;
pub const MACH_SEND_INVALID_NOTIFY:        mach_msg_return_t = 0x1000_000b;
pub const MACH_SEND_NO_NOTIFY:             mach_msg_return_t = 0x1000_000e;

// ---------------------------------------------------------------------------
//  More message-return constants used by ipc_pset
// ---------------------------------------------------------------------------

pub const MACH_RCV_PORT_CHANGED: mach_msg_return_t = 0x1000_4006;
pub const MACH_RCV_PORT_DIED:    mach_msg_return_t = 0x1000_4009;

/// `mach/message.h`: option-bit constants used by ipc_mqueue.
pub const MACH_SEND_TIMEOUT: mach_msg_option_t = 0x0000_0010;
pub const MACH_RCV_TIMEOUT:  mach_msg_option_t = 0x0000_0100;

/// `mach/message.h`: msgh_bits flags.
pub const MACH_MSGH_BITS_CIRCULAR: u32 = 0x4000_0000;

/// `mach/message.h`: send/receive return codes.
pub const MACH_SEND_IN_PROGRESS:   mach_msg_return_t = 0x1000_0001;
pub const MACH_SEND_TIMED_OUT:     mach_msg_return_t = 0x1000_0004;
pub const MACH_SEND_INTERRUPTED:   mach_msg_return_t = 0x1000_0007;
pub const MACH_RCV_IN_PROGRESS:    mach_msg_return_t = 0x1000_4001;
pub const MACH_RCV_INVALID_NAME:   mach_msg_return_t = 0x1000_4002;
pub const MACH_RCV_TIMED_OUT:      mach_msg_return_t = 0x1000_4003;
pub const MACH_RCV_TOO_LARGE:      mach_msg_return_t = 0x1000_4004;
pub const MACH_RCV_INTERRUPTED:    mach_msg_return_t = 0x1000_4005;
pub const MACH_RCV_IN_SET:         mach_msg_return_t = 0x1000_400a;
pub const MACH_RCV_HEADER_ERROR:   mach_msg_return_t = 0x1000_400b;
pub const MACH_RCV_BODY_ERROR:     mach_msg_return_t = 0x1000_400c;

/// Special-bit return-code flags (combined with `MACH_RCV_HEADER_ERROR` etc.).
pub const MACH_MSG_IPC_SPACE:      mach_msg_return_t = 0x0000_2000;
pub const MACH_MSG_VM_SPACE:       mach_msg_return_t = 0x0000_1000;
pub const MACH_MSG_IPC_KERNEL:     mach_msg_return_t = 0x0000_0800;
pub const MACH_MSG_VM_KERNEL:      mach_msg_return_t = 0x0000_0400;

/// Send-side error codes used by ipc_kmsg copyin.
pub const MACH_SEND_INVALID_DATA:    mach_msg_return_t = 0x1000_0002;
pub const MACH_SEND_INVALID_DEST:    mach_msg_return_t = 0x1000_0003;
pub const MACH_SEND_INVALID_REPLY:   mach_msg_return_t = 0x1000_0009;
pub const MACH_SEND_INVALID_RIGHT:   mach_msg_return_t = 0x1000_000a;
pub const MACH_SEND_INVALID_MEMORY:  mach_msg_return_t = 0x1000_000c;
pub const MACH_SEND_NO_BUFFER:       mach_msg_return_t = 0x1000_000d;
pub const MACH_SEND_INVALID_TYPE:    mach_msg_return_t = 0x1000_000f;
pub const MACH_SEND_INVALID_HEADER:  mach_msg_return_t = 0x1000_0010;
pub const MACH_SEND_MSG_TOO_SMALL:   mach_msg_return_t = 0x1000_0008;
pub const MACH_RCV_INVALID_DATA:     mach_msg_return_t = 0x1000_4008;
pub const MACH_RCV_INVALID_NOTIFY:   mach_msg_return_t = 0x1000_4007;

/// `mach/message.h`: `MACH_MSG_TYPE_PROTECTED_PAYLOAD = 23`.
pub const MACH_MSG_TYPE_PROTECTED_PAYLOAD: u32 = 23;

/// `mach/message.h`: alignment helpers.  KERNEL = sizeof(uintptr_t) = 4 on i686.
pub const MACH_MSG_KERNEL_ALIGNMENT: usize = 4;
pub const MACH_MSG_USER_ALIGNMENT:   usize = 4;

/// `MACH_MSGH_BITS_REMOTE/LOCAL/PORTS/OTHER` macros.
pub const MACH_MSGH_BITS_REMOTE_MASK: u32 = 0x0000_00ff;
pub const MACH_MSGH_BITS_LOCAL_MASK:  u32 = 0x0000_ff00;
pub const MACH_MSGH_BITS_PORTS_MASK:  u32 =
    MACH_MSGH_BITS_REMOTE_MASK | MACH_MSGH_BITS_LOCAL_MASK;

#[inline]
#[allow(non_snake_case)]
pub const fn MACH_MSGH_BITS_LOCAL(bits: u32) -> u32 {
    (bits & MACH_MSGH_BITS_LOCAL_MASK) >> 8
}
#[inline]
#[allow(non_snake_case)]
pub const fn MACH_MSGH_BITS_PORTS(bits: u32) -> u32 {
    bits & MACH_MSGH_BITS_PORTS_MASK
}
#[inline]
#[allow(non_snake_case)]
pub const fn MACH_MSGH_BITS_OTHER(bits: u32) -> u32 {
    bits & !MACH_MSGH_BITS_PORTS_MASK
}

/// `MACH_MSG_TYPE_PORT_ANY(x)` — true for receive/send/sonce/copy/move/make/payload.
#[inline]
#[allow(non_snake_case)]
pub const fn MACH_MSG_TYPE_PORT_ANY(x: u32) -> bool {
    x == MACH_MSG_TYPE_MOVE_RECEIVE
        || x == MACH_MSG_TYPE_MOVE_SEND
        || x == MACH_MSG_TYPE_MOVE_SEND_ONCE
        || x == MACH_MSG_TYPE_COPY_SEND
        || x == MACH_MSG_TYPE_MAKE_SEND
        || x == MACH_MSG_TYPE_MAKE_SEND_ONCE
}
/// `MACH_MSG_TYPE_PORT_ANY_SEND(x)` — port-types except MOVE_RECEIVE.
#[inline]
#[allow(non_snake_case)]
pub const fn MACH_MSG_TYPE_PORT_ANY_SEND(x: u32) -> bool {
    x == MACH_MSG_TYPE_MOVE_SEND
        || x == MACH_MSG_TYPE_MOVE_SEND_ONCE
        || x == MACH_MSG_TYPE_COPY_SEND
        || x == MACH_MSG_TYPE_MAKE_SEND
        || x == MACH_MSG_TYPE_MAKE_SEND_ONCE
}

/// `kern/sched_prim.h`: `wait_result_t` codes.
pub const THREAD_AWAKENED:   kern_return_t = 0;
pub const THREAD_TIMED_OUT:  kern_return_t = 1;
pub const THREAD_INTERRUPTED: kern_return_t = 2;
pub const THREAD_RESTART:    kern_return_t = 3;

pub const MACH_PORT_RIGHT_PORT_SET: u32 = 3;
pub const MACH_PORT_TYPE_PORT_SET: u32 =
    MACH_PORT_TYPE(MACH_PORT_RIGHT_PORT_SET);

/// `mach/kern_return.h`: `KERN_NOT_IN_SET = 12`.
pub const KERN_NOT_IN_SET: kern_return_t = 12;

/// `ipc/ipc_object.h`: `IO_BITS_ACTIVE = 0x80000000U`.
pub const IO_BITS_ACTIVE: ipc_object_bits_t = 0x8000_0000;
/// `ipc/ipc_object.h`: `IO_BITS_OTYPE = 0x3fff0000`.
pub const IO_BITS_OTYPE:  ipc_object_bits_t = 0x3fff_0000;
/// `ipc/ipc_object.h`: `IO_BITS_KOTYPE = 0x0000ffff`.
pub const IO_BITS_KOTYPE: ipc_object_bits_t = 0x0000_ffff;

/// `kern/ipc_kobject.h`: `IKOT_NONE = 0`.
pub const IKOT_NONE: u32 = 0;

/// `mach/port.h`: `MACH_PORT_QLIMIT_DEFAULT = 5`.
pub const MACH_PORT_QLIMIT_DEFAULT: u32 = 5;

// ---------------------------------------------------------------------------
//  kern/ipc_kobject.h — opaque kobject typedef
// ---------------------------------------------------------------------------

/// `kern/ipc_kobject.h`: `typedef vm_offset_t ipc_kobject_t;`
pub type ipc_kobject_t = vm_offset_t;

// ---------------------------------------------------------------------------
//  ipc/ipc_port.h — `struct ipc_port` (full mirror)
// ---------------------------------------------------------------------------

/// `union { struct ipc_space *receiver; struct ipc_port *destination;
///          ipc_port_timestamp_t timestamp; } data;`  All variants 4 bytes.
#[repr(C)]
pub union ipc_port_data {
    pub receiver: ipc_space_t,
    pub destination: *mut ipc_port,
    pub timestamp: ipc_port_timestamp_t,
}
const _: () = assert!(size_of::<ipc_port_data>() == 4);

#[repr(C)]
pub struct ipc_port {
    pub ip_target: ipc_target,           /* 0..20 */
    pub ip_cur_target: *mut ipc_target,  /* 20..24 */
    pub data: ipc_port_data,             /* 24..28 */
    pub ip_kobject: ipc_kobject_t,       /* 28..32 */
    pub ip_mscount: mach_port_mscount_t, /* 32..36 */
    pub ip_srights: u32,                 /* 36..40 */
    pub ip_sorights: u32,                /* 40..44 */
    pub ip_nsrequest: *mut ipc_port,     /* 44..48 */
    pub ip_pdrequest: *mut ipc_port,     /* 48..52 */
    pub ip_dnrequests: *mut ipc_port_request, /* 52..56 */
    pub ip_pset: *mut ipc_pset,          /* 56..60 */
    pub ip_seqno: mach_port_seqno_t,     /* 60..64 */
    pub ip_msgcount: u32,                /* 64..68 */
    pub ip_qlimit: u32,                  /* 68..72 */
    pub ip_blocked: ipc_thread_queue,    /* 72..76 */
    pub ip_protected_payload: u32,       /* 76..80 */
}

const _: () = assert!(size_of::<ipc_port>() == SIZE_OF_IPC_PORT);

// ---------------------------------------------------------------------------
//  ipc/ipc_pset.h — `struct ipc_pset` (full mirror, single field)
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct ipc_pset {
    pub ips_target: ipc_target,
}

const _: () = assert!(size_of::<ipc_pset>() == SIZE_OF_IPC_PSET);

pub const IPS_NULL: ipc_pset_t = core::ptr::null_mut();
