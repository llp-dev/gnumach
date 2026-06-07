/*
 * Mach Operating System
 * Copyright (c) 1990 Carnegie-Mellon University
 * Copyright (c) 1989 Carnegie-Mellon University
 * All rights reserved.  The CMU software License Agreement specifies
 * the terms and conditions for use and redistribution.
 */
/*
 * Copyright 1990 by Open Software Foundation,
 * Grenoble, FRANCE
 *
 * 		All Rights Reserved
 *
 *   Permission to use, copy, modify, and distribute this software and
 * its documentation for any purpose and without fee is hereby granted,
 * provided that the above copyright notice appears in all copies and
 * that both the copyright notice and this permission notice appear in
 * supporting documentation, and that the name of OSF or Open Software
 * Foundation not be used in advertising or publicity pertaining to
 * distribution of the software without specific, written prior
 * permission.
 *
 *   OSF DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE
 * INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS,
 * IN NO EVENT SHALL OSF BE LIABLE FOR ANY SPECIAL, INDIRECT, OR
 * CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
 * LOSS OF USE, DATA OR PROFITS, WHETHER IN ACTION OF CONTRACT,
 * NEGLIGENCE, OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION
 * WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

/*
 *	Support for MP lock monitoring (MACH_LOCK_MON).
 *		Registers use of locks, contention.
 *		Depending on hardware also records time spent with locks held
 */

#include <sys/types.h>
#include <string.h>

#include <mach/machine/vm_types.h>
#include <mach/boolean.h>
#include <kern/thread.h>
#include <kern/lock.h>
#include <kern/printf.h>
#include <kern/mach_clock.h>
#include <machine/ipl.h>

def_simple_lock_data(, kdb_lock)
def_simple_lock_data(, printf_lock)

#if	NCPUS > 1 && MACH_LOCK_MON
#define TIME_STAMP 1
typedef unsigned int time_stamp_t;
/* in milliseconds */
#define	time_stamp (elapsed_ticks * 1000 / hz)

#define LOCK_INFO_MAX	     (1024*32)
#define LOCK_INFO_HASH_COUNT 1024
#define LOCK_INFO_PER_BUCKET	(LOCK_INFO_MAX/LOCK_INFO_HASH_COUNT)

#define HASH_LOCK(lock)	((long)lock>>5 & (LOCK_INFO_HASH_COUNT-1))

struct lock_info {
	unsigned int	success;
	unsigned int	fail;
	unsigned int	masked;
	unsigned int	stack;
	time_stamp_t	time;
	decl_simple_lock_data(, *lock)
	vm_offset_t	caller;
};

struct lock_info_bucket {
	struct lock_info info[LOCK_INFO_PER_BUCKET];
};

struct lock_info_bucket lock_info[LOCK_INFO_HASH_COUNT];
struct lock_info default_lock_info;
unsigned default_lock_stack = 0;

extern spl_t curr_ipl[];



struct lock_info *
locate_lock_info(lock)
decl_simple_lock_data(, **lock)
{
	struct lock_info *li =  &(lock_info[HASH_LOCK(*lock)].info[0]);
	int i;

	for (i=0; i < LOCK_INFO_PER_BUCKET; i++, li++)
		if (li->lock) {
			if (li->lock == *lock)
				return(li);
		} else {
			li->lock = *lock;
			li->caller = *((vm_offset_t *)lock - 1);
			return(li);
		}
	li = &default_lock_info;
	return(li);
}


void simple_lock(lock)
decl_simple_lock_data(, *lock)
{
	struct lock_info *li = locate_lock_info(&lock);
	int my_cpu = cpu_number();

	if (current_thread())
		li->stack = current_thread()->lock_stack++;
	if (curr_ipl[my_cpu])
		li->masked++;
	if (_simple_lock_try(lock))
		li->success++;
	else {
		_simple_lock(lock);
		li->fail++;
	}
	li->time = time_stamp - li->time;
}

int simple_lock_try(lock)
decl_simple_lock_data(, *lock)
{
	struct lock_info *li = locate_lock_info(&lock);
	int my_cpu = cpu_number();

	if (curr_ipl[my_cpu])
		li->masked++;
	if (_simple_lock_try(lock)) {
		li->success++;
		li->time = time_stamp - li->time;
		if (current_thread())
			li->stack = current_thread()->lock_stack++;
		return(1);
	} else {
		li->fail++;
		return(0);
	}
}

void simple_unlock(lock)
decl_simple_lock_data(, *lock)
{
	time_stamp_t stamp = time_stamp;
	time_stamp_t *time = &locate_lock_info(&lock)->time;
	unsigned *lock_stack;

	*time = stamp - *time;
	_simple_unlock(lock);
	if (current_thread()) {
		lock_stack = &current_thread()->lock_stack;
		if (*lock_stack)
			(*lock_stack)--;
	}
}

#define lock_info_clear lic

void lock_info_clear(void)
{
	struct lock_info *li;
	int bucket = 0;
	int i;
	for (bucket = 0; bucket < LOCK_INFO_HASH_COUNT; bucket++) {
		li = &lock_info[bucket].info[0];
		for (i= 0; i< LOCK_INFO_PER_BUCKET; i++, li++) {
			memset(li, 0, sizeof(struct lock_info));
		}
	}
	memset(&default_lock_info, 0, sizeof(struct lock_info));
}

#endif	/* NCPUS > 1 && MACH_LOCK_MON */

#if	TIME_STAMP

/*
 *	Measure lock/unlock operations
 */

void time_lock(int loops)
{
	decl_simple_lock_data(, lock)
	int i;


	if (!loops)
		loops = 1000;
	simple_lock_init(&lock);
	for (i = 0; i < loops; i++) {
		simple_lock(&lock);
		simple_unlock(&lock);
	}
#if	MACH_LOCK_MON
	for (i = 0; i < loops; i++) {
		_simple_lock(&lock);
		_simple_unlock(&lock);
        }
#endif	/* MACH_LOCK_MON */
}
#endif	/* TIME_STAMP */
