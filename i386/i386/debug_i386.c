/*
 * Copyright (c) 1994 The University of Utah and
 * the Computer Systems Laboratory at the University of Utah (CSL).
 * All rights reserved.
 *
 * Permission to use, copy, modify and distribute this software is hereby
 * granted provided that (1) source code retains these copyright, permission,
 * and disclaimer notices, and (2) redistributions including binaries
 * reproduce the notices in supporting documentation, and (3) all advertising
 * materials mentioning features or use of this software display the following
 * acknowledgement: ``This product includes software developed by the
 * Computer Systems Laboratory at the University of Utah.''
 *
 * THE UNIVERSITY OF UTAH AND CSL ALLOW FREE USE OF THIS SOFTWARE IN ITS "AS
 * IS" CONDITION.  THE UNIVERSITY OF UTAH AND CSL DISCLAIM ANY LIABILITY OF
 * ANY KIND FOR ANY DAMAGES WHATSOEVER RESULTING FROM THE USE OF THIS SOFTWARE.
 *
 * CSL requests users of this software to return to csl-dist@cs.utah.edu any
 * improvements that they make and grant CSL redistribution rights.
 *
 *      Author: Bryan Ford, University of Utah CSL
 */

#include <kern/printf.h>

#include "thread.h"
#include "trap.h"
#include "debug.h"
#include "spl.h"

void dump_ss(const struct i386_saved_state *st)
{
	printf("Dump of i386_saved_state %p:\n", st);
	printf("RAX %016lx RBX %016lx RCX %016lx RDX %016lx\n",
		st->eax, st->ebx, st->ecx, st->edx);
	printf("RSI %016lx RDI %016lx RBP %016lx RSP %016lx\n",
		st->esi, st->edi, st->ebp, st->uesp);
	printf("R8  %016lx R9  %016lx R10 %016lx R11 %016lx\n",
		st->r8, st->r9, st->r10, st->r11);
	printf("R12 %016lx R13 %016lx R14 %016lx R15 %016lx\n",
		st->r12, st->r13, st->r14, st->r15);
	printf("RIP %016lx EFLAGS %08lx\n", st->eip, st->efl);
	printf("trapno %ld: %s, error %08lx\n",
		st->trapno, trap_name(st->trapno),
		st->err);
}
