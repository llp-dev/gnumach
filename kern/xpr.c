/*
 * Mach Operating System
 * Copyright (c) 1991,1990,1989,1988,1987 Carnegie Mellon University
 * All Rights Reserved.
 *
 * Permission to use, copy, modify and distribute this software and its
 * documentation is hereby granted, provided that both the copyright
 * notice and this permission notice appear in all copies of the
 * software, derivative works or modified versions, and any portions
 * thereof, and that both notices appear in supporting documentation.
 *
 * CARNEGIE MELLON ALLOWS FREE USE OF THIS SOFTWARE IN ITS "AS IS"
 * CONDITION.  CARNEGIE MELLON DISCLAIMS ANY LIABILITY OF ANY KIND FOR
 * ANY DAMAGES WHATSOEVER RESULTING FROM THE USE OF THIS SOFTWARE.
 *
 * Carnegie Mellon requests users of this software to return to
 *
 *  Software Distribution Coordinator  or  Software.Distribution@CS.CMU.EDU
 *  School of Computer Science
 *  Carnegie Mellon University
 *  Pittsburgh PA 15213-3890
 *
 * any improvements or extensions that they make and grant Carnegie Mellon
 * the rights to redistribute these changes.
 */

/*
 * xpr silent tracing circular buffer.
 */
#include <string.h>

#include <kern/debug.h>
#include <kern/xpr.h>
#include <kern/lock.h>
#include "cpu_number.h"
#include <machine/spl.h>
#include <vm/vm_kern.h>


/*
 *	After a spontaneous reboot, it is desirable to look
 *	at the old xpr buffer.  Assuming xprbootstrap allocates
 *	the buffer in the same place in physical memory and
 *	the reboot doesn't clear memory, this should work.
 *	xprptr will be reset, but the saved value should be OK.
 *	Just set xprenable false so the buffer isn't overwritten.
 */

def_simple_lock_data(static,	xprlock)

boolean_t xprenable = TRUE;	/* Enable xpr tracing */
int nxprbufs = 0;	/* Number of contiguous xprbufs allocated */
int xprflags = 0;	/* Bit mask of xpr flags enabled */
struct xprbuf *xprbase;	/* Pointer to circular buffer nxprbufs*sizeof(xprbuf)*/
struct xprbuf *xprptr;	/* Currently allocated xprbuf */
struct xprbuf *xprlast;	/* Pointer to end of circular buffer */

/*VARARGS1*/
void xpr(
	char 	*msg,
	int 	arg1, 
	int 	arg2, 
	int 	arg3, 
	int 	arg4, 
	int 	arg5)
{
	spl_t s;
	struct xprbuf *x;

	/* If we aren't initialized, ignore trace request */
	if (!xprenable || (xprptr == 0))
		return;
	/* Guard against all interrupts and allocate next buffer. */
	s = splhigh();
	simple_lock(&xprlock);
	x = xprptr++;
	if (xprptr >= xprlast) {
		/* wrap around */
		xprptr = xprbase;
	}
	/* Save xprptr in allocated memory. */
	*(struct xprbuf **)xprlast = xprptr;
	simple_unlock(&xprlock);
	splx(s);
	x->msg = msg;
	x->arg1 = arg1;
	x->arg2 = arg2;
	x->arg3 = arg3;
	x->arg4 = arg4;
	x->arg5 = arg5;
	x->timestamp = XPR_TIMESTAMP;
	x->cpuinfo = cpu_number();
}

void xprbootstrap(void)
{
	vm_offset_t addr;
	vm_size_t size;
	kern_return_t kr;

	simple_lock_init(&xprlock);
	if (nxprbufs == 0)
		return;	/* assume XPR support not desired */

	/* leave room at the end for a saved copy of xprptr */
	size = nxprbufs * sizeof(struct xprbuf) + sizeof xprptr;

	kr = kmem_alloc_wired(kernel_map, &addr, size);
	if (kr != KERN_SUCCESS)
		panic("xprbootstrap");

	if (xprenable) {
		/*
		 *	If xprenable is set (the default) then we zero
		 *	the buffer so stale entries aren't mistaken for
		 *	valid ones.  If xprenable isn't set, then we preserve
		 *	the original contents of the buffer.  This is useful
		 *	if memory survives reboots, so the previous buffer
		 *	contents can be inspected.
		 */

		memset((void *) addr, 0, size);
	}

	xprbase = (struct xprbuf *) addr;
	xprlast = &xprbase[nxprbufs];
	xprptr = xprbase;	/* setting xprptr enables tracing */
}

int		xprinitial = 0;

void xprinit(void)
{
	xprflags |= xprinitial;
}

