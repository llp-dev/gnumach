/*
 *  Copyright (C) 2023 Free Software Foundation
 *
 * This program is free software ; you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation ; either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY ; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with the program ; if not, write to the Free Software
 * Foundation, Inc., 675 Mass Ave, Cambridge, MA 02139, USA.
 */

#ifdef __LP64__

#include <stddef.h>
#include <string.h>

#include <ipc/copy_user.h>
#include <kern/debug.h>
#include <mach/boolean.h>


/* Mach field descriptors measure size in bits */
#define descsize_to_bytes(n) (n / 8)
#define bytes_to_descsize(n) (n * 8)


/*
 * Expand the msg header and, if required, the msg body (ports, pointers)
 *
 * To not make the code too complicated, we use the fact that some fields of
 * mach_msg_header have the same size in the kernel and user variant (basically
 * all fields except ports and addresses)
*/
int copyinmsg (const void *userbuf, void *kernelbuf, const size_t usize, const size_t ksize)
{
  const mach_msg_user_header_t *umsg = userbuf;
  mach_msg_header_t *kmsg = kernelbuf;

  _Static_assert(!mach_msg_user_is_misaligned(sizeof(mach_msg_user_header_t)),
                 "mach_msg_user_header_t needs to be MACH_MSG_USER_ALIGNMENT aligned.");

  /* The 64 bit interface ensures the header is the same size, so it does not need any resizing. */
  _Static_assert(sizeof(mach_msg_header_t) == sizeof(mach_msg_user_header_t),
		 "mach_msg_header_t and mach_msg_user_header_t expected to be of the same size");
  if (copyin(umsg, kmsg, usize))
    return 1;

  kmsg->msgh_size = usize;
  kmsg->msgh_remote_port &= 0xFFFFFFFF; // FIXME: still have port names here
  kmsg->msgh_local_port &= 0xFFFFFFFF;  // also, this assumes little-endian
  return 0;
}


#endif /* __LP64__ */
