/*
 * Compile-time layout assertions consumed by the Rust port under rust/ipc/.
 *
 * The Rust side hand-mirrors only the fields it touches in `#[repr(C)]`
 * structs.  Each such mirror commits to specific offsets / sizes;  if a C
 * struct gets reordered or grown in front of a mirrored field the wrong
 * memory would be touched at runtime.  These _Static_asserts catch that at
 * C compile time.
 *
 * Add an assertion when adding a new mirror in rust/ipc/mach_types.rs.
 */

#include <stddef.h>
#include <mach/message.h>
#include <mach/notify.h>
#include <kern/thread.h>
#include <kern/slab.h>
#include <kern/lock.h>
#include <kern/rdxtree.h>
#include <vm/vm_map.h>
#include <ipc/ipc_kmsg.h>
#include <ipc/ipc_object.h>
#include <ipc/ipc_entry.h>
#include <ipc/ipc_space.h>
#include <ipc/ipc_port.h>
#include <ipc/ipc_pset.h>
#include <ipc/ipc_mqueue.h>
#include <ipc/ipc_marequest.h>
#include <ipc/ipc_target.h>
#include <ipc/ipc_thread.h>
#include <kern/task.h>
#include <i386/percpu.h>

/* struct ipc_object */
_Static_assert(sizeof(struct ipc_object) == 8,
	"rust/ipc/mach_types.rs::ipc_object size drift");

/* struct ipc_kmsg_queue */
_Static_assert(sizeof(struct ipc_kmsg_queue) == 4,
	"rust/ipc/mach_types.rs::ipc_kmsg_queue size drift");

/* struct ipc_thread_queue */
_Static_assert(sizeof(struct ipc_thread_queue) == 4,
	"rust/ipc/mach_types.rs::ipc_thread_queue size drift");

/* struct ipc_mqueue */
_Static_assert(sizeof(struct ipc_mqueue) == 8,
	"rust/ipc/mach_types.rs::ipc_mqueue size drift");

/* struct ipc_target */
_Static_assert(sizeof(struct ipc_target) == 20,
	"rust/ipc/mach_types.rs::ipc_target size drift");
_Static_assert(offsetof(struct ipc_target, ipt_object) == 0,
	"rust/ipc/mach_types.rs::ipc_target.ipt_object offset drift");
_Static_assert(offsetof(struct ipc_target, ipt_name) == 8,
	"rust/ipc/mach_types.rs::ipc_target.ipt_name offset drift");
_Static_assert(offsetof(struct ipc_target, ipt_messages) == 12,
	"rust/ipc/mach_types.rs::ipc_target.ipt_messages offset drift");

/* struct thread — full field-by-field mirror.  Spot-check a few offsets and
   the total size; if any field is moved/added the size assertion catches it. */
_Static_assert(sizeof(struct thread) == 352,
	"rust/ipc/mach_types.rs::thread size drift");
_Static_assert(offsetof(struct thread, ith_next) == 116,
	"rust/ipc/mach_types.rs::thread.ith_next offset drift");
_Static_assert(offsetof(struct thread, ith_prev) == 120,
	"rust/ipc/mach_types.rs::thread.ith_prev offset drift");
_Static_assert(offsetof(struct thread, ith_state) == 124,
	"rust/ipc/mach_types.rs::thread.ith_state offset drift");
_Static_assert(offsetof(struct thread, ith_self) == 140,
	"rust/ipc/mach_types.rs::thread.ith_self offset drift");
_Static_assert(offsetof(struct thread, saved) == 160,
	"rust/ipc/mach_types.rs::thread.saved offset drift");
_Static_assert(offsetof(struct thread, name) == 320,
	"rust/ipc/mach_types.rs::thread.name offset drift");

/* struct vm_map — opaque storage in rust/ipc/ipc_init.rs (84-byte blob). */
_Static_assert(sizeof(struct vm_map) == 84,
	"rust/ipc/ipc_init.rs::ipc_kernel_map_store size drift");
_Static_assert(__alignof__(struct vm_map) == 4,
	"rust/ipc/ipc_init.rs::ipc_kernel_map_store alignment drift");

/* sizes consumed by rust/ipc/ipc_init.rs::ipc_bootstrap (kmem_cache_init) */
_Static_assert(sizeof(struct ipc_space) == 44,
	"rust/ipc/mach_types.rs::ipc_space size drift");
_Static_assert(sizeof(struct ipc_entry) == 16,
	"rust/ipc/mach_types.rs::ipc_entry size drift");
_Static_assert(sizeof(struct ipc_port) == 80,
	"rust/ipc/mach_types.rs::ipc_port size drift");
_Static_assert(offsetof(struct ipc_port, ip_target)      == 0,
	"rust/ipc/mach_types.rs::ipc_port.ip_target offset drift");
_Static_assert(offsetof(struct ipc_port, ip_cur_target)  == 20,
	"rust/ipc/mach_types.rs::ipc_port.ip_cur_target offset drift");
_Static_assert(offsetof(struct ipc_port, ip_kobject)     == 28,
	"rust/ipc/mach_types.rs::ipc_port.ip_kobject offset drift");
_Static_assert(offsetof(struct ipc_port, ip_mscount)     == 32,
	"rust/ipc/mach_types.rs::ipc_port.ip_mscount offset drift");
_Static_assert(offsetof(struct ipc_port, ip_pset)        == 56,
	"rust/ipc/mach_types.rs::ipc_port.ip_pset offset drift");
_Static_assert(offsetof(struct ipc_port, ip_seqno)       == 60,
	"rust/ipc/mach_types.rs::ipc_port.ip_seqno offset drift");
_Static_assert(offsetof(struct ipc_port, ip_protected_payload) == 76,
	"rust/ipc/mach_types.rs::ipc_port.ip_protected_payload offset drift");

_Static_assert(sizeof(struct ipc_pset) == 20,
	"rust/ipc/mach_types.rs::ipc_pset size drift");
_Static_assert(offsetof(struct ipc_pset, ips_target)     == 0,
	"rust/ipc/mach_types.rs::ipc_pset.ips_target offset drift");
_Static_assert(sizeof(struct kmem_cache) == 128,
	"rust/ipc/mach_types.rs::kmem_cache size drift");

/* mach_msg_header_t / mach_msg_type_t / notification structs / ipc_kmsg
   — consumed by rust/ipc/ipc_notify.rs and friends. */
_Static_assert(sizeof(mach_msg_header_t) == 24,
	"rust/ipc/mach_types.rs::mach_msg_header_t size drift");
_Static_assert(sizeof(mach_msg_type_t) == 4,
	"rust/ipc/mach_types.rs::mach_msg_type_t size drift");
_Static_assert(sizeof(mach_port_deleted_notification_t) == 32,
	"rust/ipc/mach_types.rs::mach_port_deleted_notification_t size drift");
_Static_assert(sizeof(mach_msg_accepted_notification_t) == 32,
	"rust/ipc/mach_types.rs::mach_msg_accepted_notification_t size drift");
_Static_assert(sizeof(mach_port_destroyed_notification_t) == 32,
	"rust/ipc/mach_types.rs::mach_port_destroyed_notification_t size drift");
_Static_assert(sizeof(mach_no_senders_notification_t) == 32,
	"rust/ipc/mach_types.rs::mach_no_senders_notification_t size drift");
_Static_assert(sizeof(mach_send_once_notification_t) == 24,
	"rust/ipc/mach_types.rs::mach_send_once_notification_t size drift");
_Static_assert(sizeof(mach_dead_name_notification_t) == 32,
	"rust/ipc/mach_types.rs::mach_dead_name_notification_t size drift");
_Static_assert(sizeof(struct ipc_kmsg) == 40,
	"rust/ipc/mach_types.rs::ipc_kmsg_full size drift");

/* ipc_entry / ipc_space / ipc_marequest field-by-field mirrors. */
_Static_assert(offsetof(struct ipc_entry, ie_name)   == 0,
	"rust/ipc/mach_types.rs::ipc_entry.ie_name offset drift");
_Static_assert(offsetof(struct ipc_entry, ie_bits)   == 4,
	"rust/ipc/mach_types.rs::ipc_entry.ie_bits offset drift");
_Static_assert(offsetof(struct ipc_entry, ie_object) == 8,
	"rust/ipc/mach_types.rs::ipc_entry.ie_object offset drift");
_Static_assert(offsetof(struct ipc_entry, index)     == 12,
	"rust/ipc/mach_types.rs::ipc_entry.index offset drift");

_Static_assert(offsetof(struct ipc_space, is_active)        == 12,
	"rust/ipc/mach_types.rs::ipc_space.is_active offset drift");
_Static_assert(offsetof(struct ipc_space, is_map)           == 16,
	"rust/ipc/mach_types.rs::ipc_space.is_map offset drift");
_Static_assert(offsetof(struct ipc_space, is_size)          == 24,
	"rust/ipc/mach_types.rs::ipc_space.is_size offset drift");
_Static_assert(offsetof(struct ipc_space, is_reverse_map)   == 28,
	"rust/ipc/mach_types.rs::ipc_space.is_reverse_map offset drift");
_Static_assert(offsetof(struct ipc_space, is_free_list)     == 36,
	"rust/ipc/mach_types.rs::ipc_space.is_free_list offset drift");
_Static_assert(offsetof(struct ipc_space, is_free_list_size) == 40,
	"rust/ipc/mach_types.rs::ipc_space.is_free_list_size offset drift");

_Static_assert(sizeof(struct ipc_marequest) == 16,
	"rust/ipc/mach_types.rs::ipc_marequest_full size drift");
_Static_assert(offsetof(struct ipc_marequest, imar_space)   == 0,
	"rust/ipc/mach_types.rs::ipc_marequest_full.imar_space offset drift");
_Static_assert(offsetof(struct ipc_marequest, imar_name)    == 4,
	"rust/ipc/mach_types.rs::ipc_marequest_full.imar_name offset drift");
_Static_assert(offsetof(struct ipc_marequest, imar_soright) == 8,
	"rust/ipc/mach_types.rs::ipc_marequest_full.imar_soright offset drift");
_Static_assert(offsetof(struct ipc_marequest, imar_next)    == 12,
	"rust/ipc/mach_types.rs::ipc_marequest_full.imar_next offset drift");

_Static_assert(sizeof(struct lock) == 8,
	"rust/ipc/mach_types.rs::lock_data_t size drift");
_Static_assert(sizeof(struct rdxtree) == 8,
	"rust/ipc/mach_types.rs::rdxtree size drift");

/* struct task — fields needed by Rust ipc_mqueue.c port (statistics
   counters).  We use the offset from C in a sized-blob mirror Rust-side. */
_Static_assert(__builtin_offsetof(struct task, messages_sent) == 160,
	"rust/ipc/mach_types.rs::task.messages_sent offset drift");
_Static_assert(__builtin_offsetof(struct task, messages_received) == 164,
	"rust/ipc/mach_types.rs::task.messages_received offset drift");
_Static_assert(__builtin_offsetof(struct task, map) == 8,
	"rust/ipc/mach_types.rs::task.map offset drift");
_Static_assert(__builtin_offsetof(struct task, itk_space) == 128,
	"rust/ipc/mach_types.rs::task.itk_space offset drift");
_Static_assert(__builtin_offsetof(struct task, name) == 168,
	"rust/ipc/mach_types.rs::task.name offset drift");
_Static_assert(sizeof(((struct task *)0)->name) == 32,
	"rust/ipc/mach_types.rs::TASK_NAME_SIZE drift");

/* struct percpu — needed for current_thread() lookup. */
_Static_assert(__builtin_offsetof(struct percpu, active_thread) == 600,
	"rust/ipc/mach_types.rs::percpu.active_thread offset drift");

/* mach_msg_header_t / mach_msg_user_header_t — message header.  Walked by
   the IPC port via raw byte offsets; size must match Rust mirror. */
_Static_assert(sizeof(mach_msg_header_t) == 24,
	"rust/ipc/mach_types.rs::mach_msg_header_t size drift");
_Static_assert(sizeof(mach_msg_user_header_t) == 24,
	"rust/ipc/mach_types.rs::mach_msg_user_header size drift");

/* mach_msg_type_t / mach_msg_type_long_t — descriptors walked by the body
   walker in rust/ipc/ipc_kmsg.rs (ipc_kmsg_copyin_body / copyout_body).
   Wrong size = wrong stride = data corruption. */
_Static_assert(sizeof(mach_msg_type_t) == 4,
	"rust/ipc/mach_types.rs::mach_msg_type_t size drift");
_Static_assert(sizeof(mach_msg_type_long_t) == 12,
	"rust/ipc/mach_types.rs::mach_msg_type_long_t size drift");

/* struct ipc_port_request — embedded in struct ipc_port via ip_dnrequests
   and walked by the dnrequest helpers. */
_Static_assert(sizeof(struct ipc_port_request) == 8,
	"rust/ipc/mach_types.rs::ipc_port_request size drift");

/* struct rdxtree_iter — used by rdxtree walks in rust/ipc/ipc_space.rs and
   rust/ipc/mach_debug.rs. */
_Static_assert(sizeof(struct rdxtree_iter) == 8,
	"rust/ipc/mach_types.rs::rdxtree_iter size drift");
