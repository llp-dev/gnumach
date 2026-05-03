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

#ifndef COPY_USER_H
#define COPY_USER_H

#include <stdint.h>
#include <sys/types.h>

#include <machine/locore.h>
#include <mach/message.h>

static inline int copyin_address(const rpc_vm_offset_t *uaddr, vm_offset_t *kaddr)
{
  return copyin(uaddr, kaddr, sizeof(*uaddr));
}

static inline int copyout_address(const vm_offset_t *kaddr, rpc_vm_offset_t *uaddr)
{
  return copyout(kaddr, uaddr, sizeof(*kaddr));
}

static inline int copyin_port(const mach_port_name_t *uaddr, mach_port_t *kaddr)
{
  return copyin(uaddr, kaddr, sizeof(*uaddr));
}

static inline int copyout_port(const mach_port_t *kaddr, mach_port_name_t *uaddr)
{
  return copyout(kaddr, uaddr, sizeof(*kaddr));
}

static inline size_t msg_usize(const mach_msg_header_t *kmsg)
{
  return kmsg->msgh_size;
}

#endif /* COPY_USER_H */
