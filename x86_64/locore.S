/*
 * Mach Operating System
 * Copyright (c) 1993,1992,1991,1990 Carnegie Mellon University
 * Copyright (c) 1991 IBM Corporation
 * All Rights Reserved.
 *
 * Permission to use, copy, modify and distribute this software and its
 * documentation is hereby granted, provided that both the copyright
 * notice and this permission notice appear in all copies of the
 * software, derivative works or modified versions, and any portions
 * thereof, and that both notices appear in supporting documentation,
 * and that the nema IBM not be used in advertising or publicity
 * pertaining to distribution of the software without specific, written
 * prior permission.
 *
 * CARNEGIE MELLON AND IBM ALLOW FREE USE OF THIS SOFTWARE IN ITS "AS IS"
 * CONDITION.  CARNEGIE MELLON AND IBM DISCLAIM ANY LIABILITY OF ANY KIND FOR
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

#include <mach/machine/asm.h>
#include <mach/machine/eflags.h>
#include <i386/i386/proc_reg.h>
#include <i386/i386/trap.h>
#include <i386/i386/seg.h>
#include <i386/i386/gdt.h>
#include <i386/i386/ldt.h>
#include <i386/i386/msr.h>
#include <i386/i386/i386asm.h>
#include <i386/i386/cpu_number.h>


/*
 * Helpers for thread state as saved in the pcb area in i386_saved_state, during trap or irq handling
 */
#define pusha		\
	pushq %rax	;\
	pushq %rcx	;\
	pushq %rdx	;\
	pushq %rbx	;\
	subq $8,%rsp	;\
	pushq %rbp	;\
	pushq %rsi	;\
	pushq %rdi	;\
	pushq %r8	;\
	pushq %r9	;\
	pushq %r10	;\
	pushq %r11	;\
	pushq %r12	;\
	pushq %r13	;\
	pushq %r14	;\
	pushq %r15

#define popa		\
	popq %r15	;\
	popq %r14	;\
	popq %r13	;\
	popq %r12	;\
	popq %r11	;\
	popq %r10	;\
	popq %r9	;\
	popq %r8	;\
	popq %rdi	;\
	popq %rsi	;\
	popq %rbp	;\
	addq $8,%rsp	;\
	popq %rbx	;\
	popq %rdx	;\
	popq %rcx	;\
	popq %rax

/* finish filling i386_interrupt_state */
#define PUSH_REGS_ISR	\
	pushq	%rcx    ;\
	pushq	%rdx    ;\
	pushq	%rsi    ;\
	pushq	%rdi    ;\
	pushq	%r8     ;\
	pushq	%r9     ;\
	pushq	%r10     ;\
	pushq	%r11	;\
	pushq	%r12

#define PUSH_AREGS_ISR	\
	pushq	%rax    ;\
	PUSH_REGS_ISR


#define POP_REGS_ISR	\
	popq	%r12	;\
	popq	%r11    ;\
	popq	%r10     ;\
	popq	%r9     ;\
	popq	%r8     ;\
	popq	%rdi    ;\
	popq	%rsi    ;\
	popq	%rdx    ;\
	popq	%rcx

#define POP_AREGS_ISR	\
	POP_REGS_ISR	;\
	popq	%rax

/*
 * Note that we have to load the kernel segment registers even if this
 * is a trap from the kernel, because the kernel uses user segment
 * registers for copyin/copyout.
 * (XXX Would it be smarter just to use fs or gs for that?)
 */
#define PUSH_SEGMENTS(reg)

#define POP_SEGMENTS(reg)

#define PUSH_SEGMENTS_ISR(reg)

#define POP_SEGMENTS_ISR(reg)

#define SET_KERNEL_SEGMENTS(reg)

#define RETURN_TO_KERN	0x7eadbeef
#define RETURN_TO_USER	0x66666666

/* Keeps %r12 (callee-saved) value throughout interrupt context */
# define SWAPGS_ENTRY_IF_NEEDED_R12	\
	pushf				;\
	cli				;\
	pushq	%rax			;\
	pushq	%rcx			;\
	pushq	%rdx			;\
	movl	$MSR_REG_GSBASE, %ecx	;\
	rdmsr				;\
	testl	%edx, %edx	/* gs base sign bit set ? */ ;\
	js	0f		/* yes, dont swap then return to kernel mode */ ;\
	swapgs			/* no, swap then return to user mode */ ;\
	movq	$RETURN_TO_USER, %r12	;\
	jmp	1f			;\
0:	movq	$RETURN_TO_KERN, %r12	;\
1:	popq	%rdx			;\
	popq	%rcx			;\
	popq	%rax			;\
	popf

# define SWAPGS_EXIT_IF_NEEDED_R12	\
	cmpq	$RETURN_TO_USER, %r12 	;\
	je	0f		/* return to user with swap */ ;\
	cmpq	$RETURN_TO_KERN, %r12	;\
	je	1f		/* return to kern without swap */ ;\
	ud2			/* or die */ ;\
0:	swapgs				;\
1:


/*
 * Fault recovery.
 */
#define	RECOVER_TABLE_START	\
	.text	2		;\
DATA(recover_table)		;\
	.text

#define	RECOVER(addr)		\
	.text	2		;\
	.quad	9f		;\
	.quad	addr		;\
	.text			;\
9:

#define	RECOVER_TABLE_END		\
	.text	2			;\
	.globl	EXT(recover_table_end)	;\
LEXT(recover_table_end)			;\
	.text

/*
 * Retry table for certain successful faults.
 */
#define	RETRY_TABLE_START	\
	.text	3		;\
DATA(retry_table)		;\
	.text

#define	RETRY(addr)		\
	.text	3		;\
	.quad	9f		;\
	.quad	addr		;\
	.text			;\
9:

#define	RETRY_TABLE_END			\
	.text	3			;\
	.globl	EXT(retry_table_end)	;\
LEXT(retry_table_end)			;\
	.text

/*
 * Allocate recovery and retry tables.
 */
	RECOVER_TABLE_START
	RETRY_TABLE_START

/*
 * Timing routines.
 */

#define	TIME_TRAP_UENTRY
#define	TIME_TRAP_SENTRY
#define	TIME_TRAP_UEXIT
#define	TIME_INT_ENTRY
#define	TIME_INT_EXIT


/**/

/*
 * Trap/interrupt entry points.
 *
 * All traps must create the i386_saved_state struct on the stack on
 * entry. Note that:
 *      - CR2 is only used if the trap is a page fault
 *      - user_rsp/user_ss are only used if entering from user space
 *      - v86_regs are used only from V86 threads
 *         (TODO check if V86 is still used with USER32)
 *
 * Depending the CPL before entry, the stack might be switched or not;
 * if entering from user-space the CPU loads TSS->RSP0 in RSP,
 * otherwise RSP is unchanged. After this, the cpu pushes
 * SS/RSP/RFLAFS/CS/RIP and optionally ErrorCode and executes the handler.
 */

/* Try to save/show some information when a double fault happens
 * We can't recover to a working state, so if we have a debugger wait for it,
 * otherwise reset */
ENTRY(t_dbl_fault)
	INT_FIX
	cli	/* disable interrupts that might corrupt the state*/
	pushq	$(T_DOUBLE_FAULT)	/* indicate fault type  */
	pusha
	movq	%cr2,%rax
	movq	%rax,R_CR2-R_R15(%rsp)	/* CR2 might contain the faulting address */
	movq	%rsp,%rdi		/* pass the saved state */
	call	handle_double_fault
	jmp	cpu_shutdown	/* reset */
END(t_dbl_fault)

/*
 * General protection or segment-not-present fault.
 * Check for a GP/NP fault in the kernel_return
 * sequence; if there, report it as a GP/NP fault on the user's instruction.
 *
 * rsp->     0:	trap code (NP or GP)
 *	     8:	segment number in error
 *	    16	eip
 *	    24	cs
 *	    32	eflags
 *	    40	old registers (trap is from kernel)
 */
ENTRY(t_gen_prot)
	INT_FIX
	pushq	$(T_GENERAL_PROTECTION)	/* indicate fault type  */
	jmp	trap_check_kernel_exit	/* check for kernel exit sequence */

ENTRY(t_segnp)
	INT_FIX
	pushq	$(T_SEGMENT_NOT_PRESENT)
					/* indicate fault type */

trap_check_kernel_exit:
	/* Note: handling KERNEL_RING value by hand */
	testq	$2,24(%rsp)		/* is trap from kernel mode? */
	jnz	EXT(alltraps)		/* if so:  */
					/* check for the kernel exit sequence */
	cmpq	$_kret_iret,16(%rsp)	/* on IRET? */
	je	fault_iret
take_fault:				/* if none of the above: */
	jmp	EXT(alltraps)		/* treat as normal trap. */

/*
 * GP/NP fault on IRET: CS or SS is in error.
 * All registers contain the user's values.
 *
 * on SP is
 *  0	trap number
 *  8	errcode
 * 16	eip
 * 24	cs		--> trapno
 * 32	efl		--> errcode
 * 40	user eip
 * 48	user cs
 * 56	user eflags
 * 64	user rsp
 * 72	user ss
 */
fault_iret:
	movq	%rax,16(%rsp)		/* save eax (we don`t need saved eip) */
	popq	%rax			/* get trap number */
	movq	%rax,24-8(%rsp)		/* put in user trap number */
	popq	%rax			/* get error code */
	movq	%rax,32-16(%rsp)	/* put in user errcode */
	popq	%rax			/* restore eax */
	jmp	EXT(alltraps)		/* take fault */

push_segregs:
	movq	%rax,R_TRAPNO(%rsp)	/* set trap number */
	movq	%rdx,R_ERR(%rsp)	/* set error code */
	jmp	trap_set_segs		/* take trap */

/*
 * Debug trap.  Check for single-stepping across system call into
 * kernel.  If this is the case, taking the debug trap has turned
 * off single-stepping - save the flags register with the trace
 * bit set.
 */
ENTRY(t_debug)
	INT_FIX
	/* Note: handling KERNEL_RING value by hand */
	testq	$2,8(%rsp)		/* is trap from kernel mode? */
	jnz	0f			/* if so: */
	// TODO: implement the 64-bit case
	ud2
0:	pushq	$0			/* otherwise: */
	pushq	$(T_DEBUG)		/* handle as normal */
	jmp	EXT(alltraps)		/* debug fault */

/*
 * Page fault traps save cr2.
 */
ENTRY(t_page_fault)
	INT_FIX
	pushq	$(T_PAGE_FAULT)		/* mark a page fault trap */
	pusha				/* save the general registers */
	movq	%cr2,%rax		/* get the faulting address */
	movq	%rax,R_CR2-R_R15(%rsp)	/* save in rsp save slot */
	jmp	trap_push_segs		/* continue fault */

/*
 * All 'exceptions' enter here with:
 *	rsp->   trap number
 *		error code
 *		old eip
 *		old cs
 *		old eflags
 *		old rsp		if trapped from user
 *		old ss		if trapped from user
 */
ENTRY(alltraps)
	pusha				/* save the general registers */
trap_push_segs:
	PUSH_SEGMENTS(%rax)		/* and the segment registers */
	SWAPGS_ENTRY_IF_NEEDED_R12
	SET_KERNEL_SEGMENTS(%rax)	/* switch to kernel data segment */
trap_set_segs:
	cld				/* clear direction flag */
	/* Note: handling KERNEL_RING value by hand */
	testb	$2,R_CS(%rsp)		/* user mode trap? */
	jz	trap_from_kernel	/* kernel trap if not */
trap_from_user:

	CPU_NUMBER(%edx)
	TIME_TRAP_UENTRY

	movq	CX8(EXT(kernel_stack),%rdx),%rbx
	xchgq	%rbx,%rsp		/* switch to kernel stack */
					/* user regs pointer already set */
_take_trap:
	movq	%rbx,%rdi		/* pass register save area to trap */
	call	EXT(user_trap)		/* call user trap routine */
1:
	movq	(%rsp),%rsp		/* switch back to PCB stack */

/*
 * Return from trap or system call, checking for ASTs.
 * On PCB stack.
 */

_return_from_trap:
	CPU_NUMBER(%edx)
	cmpq	$0,CX8(EXT(need_ast),%rdx)
	jz	_return_to_user		/* if we need an AST: */

	movq	CX8(EXT(kernel_stack),%rdx),%rsp
					/* switch to kernel stack */
	call	EXT(i386_astintr)	/* take the AST */
	popq	%rsp			/* switch back to PCB stack */
	jmp	_return_from_trap	/* and check again (rare) */
					/* ASTs after this point will */
					/* have to wait */

_return_to_user:
	TIME_TRAP_UEXIT

/*
 * Return from kernel mode to interrupted thread.
 */

_return_from_kernel:
	SWAPGS_EXIT_IF_NEEDED_R12
	popa				/* restore general registers */
	addq	$16,%rsp		/* discard trap number and error code */
_kret_iret:
	iretq				/* return from interrupt */


/*
 * Trap from kernel mode.  No need to switch stacks.
 */
trap_from_kernel:
	movq	%rsp,%rdi		/* pass parameter */
	call	EXT(kernel_trap)	/* to kernel trap routine */

	jmp	_return_from_kernel


/*
 *	Called as a function, makes the current thread
 *	return from the kernel as if from an exception.
 */

ENTRY(thread_exception_return)
ENTRY(thread_bootstrap_return)
	movq	%rsp,%rcx			/* get kernel stack */
	or	$(KERNEL_STACK_SIZE-1),%rcx
	movq	-7-IKS_SIZE(%rcx),%rsp		/* switch back to PCB stack */
	movq	$RETURN_TO_USER, %r12
	jmp	_return_from_trap

/*
 *	Called as a function, makes the current thread
 *	return from the kernel as if from a syscall.
 *	Takes the syscall's return code as an argument.
 */

ENTRY(thread_syscall_return)
	movq	S_ARG0,%rax			/* get return value */
	movq	%rsp,%rcx			/* get kernel stack */
	or	$(KERNEL_STACK_SIZE-1),%rcx
	movq	-7-IKS_SIZE(%rcx),%rsp		/* switch back to PCB stack */
	movq	%rax,R_EAX(%rsp)		/* save return value */
	movq	$RETURN_TO_USER, %r12
	jmp	_return_from_trap

ENTRY(call_continuation)
	movq	S_ARG0,%rax			/* get continuation */
	movq	%rsp,%rcx			/* get kernel stack */
	or	$(KERNEL_STACK_SIZE-1),%rcx
	addq	$(-7-IKS_SIZE),%rcx
	movq	%rcx,%rsp			/* pop the stack */
	xorq	%rbp,%rbp			/* zero frame pointer */
	pushq	$0				/* Dummy return address */
	jmp	*%rax				/* goto continuation */

/* IOAPIC has 24 interrupts, put spurious in the same array */

#define INTERRUPT(n)				\
	.data	2				;\
	.quad	0f				;\
	.text					;\
	P2ALIGN(TEXT_ALIGN)			;\
0:						;\
	INT_FIX					;\
	pushq	%rax				;\
	movq	$(n),%rax			;\
	jmp	EXT(all_intrs)

	.data	2
DATA(int_entry_table)
	.text
/* Legacy APIC interrupts or PIC interrupts */
INTERRUPT(0)
INTERRUPT(1)
INTERRUPT(2)
INTERRUPT(3)
INTERRUPT(4)
INTERRUPT(5)
INTERRUPT(6)
INTERRUPT(7)
INTERRUPT(8)
INTERRUPT(9)
INTERRUPT(10)
INTERRUPT(11)
INTERRUPT(12)
INTERRUPT(13)
INTERRUPT(14)
INTERRUPT(15)
#ifdef APIC
/* APIC PCI interrupts PIRQ A-H */
INTERRUPT(16)
INTERRUPT(17)
INTERRUPT(18)
INTERRUPT(19)
INTERRUPT(20)
INTERRUPT(21)
INTERRUPT(22)
INTERRUPT(23)
/* Possibly 8 more GSIs */
INTERRUPT(24)
INTERRUPT(25)
INTERRUPT(26)
INTERRUPT(27)
INTERRUPT(28)
INTERRUPT(29)
INTERRUPT(30)
INTERRUPT(31)
/* ... APIC IOAPIC #2 */
INTERRUPT(32)
INTERRUPT(33)
INTERRUPT(34)
INTERRUPT(35)
INTERRUPT(36)
INTERRUPT(37)
INTERRUPT(38)
INTERRUPT(39)
INTERRUPT(40)
INTERRUPT(41)
INTERRUPT(42)
INTERRUPT(43)
INTERRUPT(44)
INTERRUPT(45)
INTERRUPT(46)
INTERRUPT(47)
INTERRUPT(48)
INTERRUPT(49)
INTERRUPT(50)
INTERRUPT(51)
INTERRUPT(52)
INTERRUPT(53)
INTERRUPT(54)
INTERRUPT(55)
/* Possibly 8 more GSIs */
INTERRUPT(56)
INTERRUPT(57)
INTERRUPT(58)
INTERRUPT(59)
INTERRUPT(60)
INTERRUPT(61)
INTERRUPT(62)
INTERRUPT(63)
#endif
#if NCPUS > 1
INTERRUPT(CALL_AST_CHECK)
INTERRUPT(CALL_PMAP_UPDATE)
#endif
#ifdef APIC
/* Spurious interrupt, set irq number to vect number */
INTERRUPT(255)
#endif

/* XXX handle NMI - at least print a warning like Linux does.  */

/*
 * All interrupts enter here. The cpu might have loaded a new RSP,
 * depending on the previous CPL, as in alltraps.
 * Old %eax on stack, interrupt number in %eax; we need to fill the remaining
 * fields of struct i386_interrupt_state, which might be in the pcb or in the
 * interrupt stack.
 */
ENTRY(all_intrs)
	PUSH_REGS_ISR			/* save registers */
	cld				/* clear direction flag */
	PUSH_SEGMENTS_ISR(%rdx)		/* save segment registers */
	SWAPGS_ENTRY_IF_NEEDED_R12
	SET_KERNEL_SEGMENTS(%rdx)	/* switch to kernel segments */

	CPU_NUMBER(%ecx)
	movq	%rsp,%rdx		/* on an interrupt stack? */
	and	$(~(INTSTACK_SIZE-1)),%rdx
	cmpq	%ss:CX8(EXT(int_stack_base),%rcx),%rdx
	je	int_from_intstack	/* if not: */

	CPU_NUMBER(%edx)

	movq	CX8(EXT(int_stack_top),%rdx),%rcx

	xchgq	%rcx,%rsp		/* switch to interrupt stack */

	pushq	%rcx			/* save pointer to old stack */


	call	EXT(interrupt)		/* call generic interrupt routine */
	.globl	EXT(return_to_iret)	/* ( label for hardclock */
LEXT(return_to_iret)			/* to find the return from calling interrupt) */

	CPU_NUMBER(%edx)


	popq	%rsp			/* switch back to old stack */

	/* Note: handling KERNEL_RING value by hand */
	testb	$2,I_CS(%rsp)		/* user mode, */
	jz	1f			/* check for ASTs */
0:
	cmpq	$0,CX8(EXT(need_ast),%rdx)
	jnz	ast_from_interrupt	/* take it if so */
1:
	SWAPGS_EXIT_IF_NEEDED_R12
	POP_SEGMENTS_ISR(%rdx)		/* restore segment regs */
	POP_AREGS_ISR			/* restore registers */

	iretq				/* return to caller */

int_from_intstack:
	CPU_NUMBER(%edx)
	cmpq	CX8(EXT(int_stack_base),%rdx),%rsp /* seemingly looping? */
	jb	stack_overflowed	/* if not: */
	call	EXT(interrupt)		/* call interrupt routine */
_return_to_iret_i:
	SWAPGS_EXIT_IF_NEEDED_R12
	POP_SEGMENTS_ISR(%rdx)
	POP_AREGS_ISR			/* restore registers */
					/* no ASTs */
	iretq

stack_overflowed:
	ud2

/*
 *	Take an AST from an interrupt.
 *	On PCB stack.
 * sp->	gs	-> edx
 *	fs	-> ecx
 *	es	-> eax
 *	ds	-> trapno
 *	edx	-> code
 *	ecx
 *	eax
 *	eip
 *	cs
 *	efl
 *	rsp
 *	ss
 */
ast_from_interrupt:
	POP_SEGMENTS_ISR(%rdx)		/* restore all registers ... */
	POP_AREGS_ISR
	pushq	$0			/* zero code */
	pushq	$0			/* zero trap number */
	pusha				/* save general registers */
	PUSH_SEGMENTS_ISR(%rdx)		/* save segment registers */
	SET_KERNEL_SEGMENTS(%rdx)	/* switch to kernel segments */
	CPU_NUMBER(%edx)
	TIME_TRAP_UENTRY

	movq	CX8(EXT(kernel_stack),%rdx),%rsp
					/* switch to kernel stack */
	call	EXT(i386_astintr)	/* take the AST */
	popq	%rsp			/* back to PCB stack */
	movq	$RETURN_TO_USER, %r12	/* definitely returning to user mode */
	jmp	_return_from_trap	/* return */




/* Entry point for 64-bit syscalls.
 * On entry we're still on the user stack, so better not use it. Instead we
 * save the thread state immediately in thread->pcb->iss, then try to invoke
 * the syscall.
 * Note: emulated syscalls seem to not be used anymore in GNU/Hurd, so they
 * are not handled here.
 * Note: added complication: need gs base to be in kernel mode during execution
 * to read the active thread twice.  Call swapgs twice, once at start and at end.
 * TODO:
     - for now we assume the return address is canonical, but apparently there
       can be cases where it's not (see how Linux handles this). Does it apply
       here?
     - check that the case where a task is suspended, and later returns via
       iretq from return_from_trap, works fine in all combinations
 */
ENTRY(syscall64)
	/* interrupts are already disabled */
	swapgs
	/* RFLAGS[32:63] are reserved, so combine syscall num (32 bit) and
	 * eflags in RAX to allow using r11 as temporary register
	 */
	shlq	$32,%r11
	shlq	$32,%rax	/* make sure bits 32:63 of %rax are zero */
	shrq	$32,%rax
	or	%r11,%rax

	/* Save thread state in pcb->iss, as on exception entry.
	 * Since this is triggered synchronously from userspace, we could
	 * save only the callee-preserved status according to the C ABI,
	 * plus RIP and EFLAGS for sysret
	 */
	movq	MY(ACTIVE_THREAD),%r11	/* point to current thread */
	movq	TH_PCB(%r11),%r11		/* point to pcb */
	addq	$ PCB_ISS,%r11			/* point to saved state */

	mov	%rsp,R_UESP(%r11)	/* callee-preserved register */
	mov	%rcx,R_EIP(%r11)	/* syscall places user RIP in RCX */
	mov	%rbx,R_EBX(%r11)	/* callee-preserved register */
	mov	%rax,%rbx		/* Now we can unpack eflags again */
	shr	$32,%rbx
	mov	%rbx,R_EFLAGS(%r11)	/* ... and save them in pcb as well */
	mov	%rbp,R_EBP(%r11)	/* callee-preserved register */
	mov	%r12,R_R12(%r11)	/* callee-preserved register */
	mov	%r13,R_R13(%r11)	/* callee-preserved register */
	mov	%r14,R_R14(%r11)	/* callee-preserved register */
	mov	%r15,R_R15(%r11)	/* callee-preserved register */

	/* Save syscall number and args for SYSCALL_EXAMINE/MSG_EXAMINE in glibc.
	 * Note: syscall number is only 32 bit, in EAX, so we sign-extend it in
	 * RAX to mask the EFLAGS bits.
	 */
	cdqe				/* sign-extend EAX in RAX */
	mov	%rax,R_EAX(%r11)	/* syscall number */
	mov	%rdi,R_EDI(%r11)	/* syscall arg0 */
	mov	%rsi,R_ESI(%r11)	/* syscall arg1 */
	mov	%rdx,R_EDX(%r11)	/* syscall arg2 */
	mov	%r10,R_R10(%r11)	/* syscall arg3 */
	mov	%r8,R_R8(%r11)		/* syscall arg4 */
	mov	%r9,R_R9(%r11)		/* syscall arg5 */

	mov	%r11,%rbx		/* prepare for error handling */
	mov	%r10,%rcx		/* fix arg3 location according to C ABI */

	/* switch to kernel stack then enable interrupts */
	CPU_NUMBER(%r11d)		/* we can call the fast version here */
	movq	CX8(EXT(kernel_stack),%r11),%rsp
	sti

	/* Now we have saved state and args 1-6 are in place.
	 * Before invoking the syscall we do some bound checking and,
	 * if we have more that 6 arguments, we need to copy the
	 * remaining ones to the kernel stack, handling page faults when
	 * accessing the user stack.
	 */
	negl	%eax			/* get system call number */
	jl	_syscall64_range	/* out of range if it was positive */
	cmpl	EXT(mach_trap_count),%eax	/* check system call table bounds */
	jg	_syscall64_range	/* error if out of range */
	shll	$5,%eax			/* manual indexing of mach_trap_t */

	/* check if we need to place some arguments on the stack */
_syscall64_args_stack:
	mov	EXT(mach_trap_table)(%rax),%r10	/* get number of arguments */
	subq	$6,%r10			/* the first 6 args are already in place */
	jle	_syscall64_call		/* skip argument copy if num args <= 6 */

	movq	R_UESP(%rbx),%r11	/* get user stack pointer */
	addq	$8,%r11			/* Skip user return address */

	lea	(%r11,%r10,8),%r11	/* point past last argument */

	movq	$VM_MAX_ADDRESS, %r12
	cmpq	%r12,%r11		/* Check segment limit by hand */
	jae	_syscall64_addr_push

0:	subq	$8,%r11
	RECOVER(_syscall64_addr_push)
	mov	(%r11),%r12
	pushq	%r12			/* push argument on stack */
	dec	%r10
	jnz	0b			/* loop for all remaining arguments */

_syscall64_call:
	call	*EXT(mach_trap_table)+8(%rax)  /* call procedure */

_syscall64_check_for_ast:
	/* Check for ast. */
	CPU_NUMBER(%r11d)

	cmpq	$0,CX8(EXT(need_ast),%r11)
	jz	_syscall64_restore_state

	/* Save the syscall return value, both on our stack, for the case
	 * i386_astintr returns normally, and in the PCB stack, in case it
	 * instead calls thread_block(thread_exception_return).
	 */
	pushq	%rax				/* save the return value on our stack */
	pushq	$0				/* dummy value to keep the stack aligned */

	/* Find the PCB stack. */
	movq	%rsp,%rcx
	or	$(KERNEL_STACK_SIZE-1),%rcx
	movq	-7-IKS_SIZE(%rcx),%rcx

	movq	%rax,R_EAX(%rcx)		/* save the return value in the PCB stack */
	call	EXT(i386_astintr)
	popq	%rax
	popq	%rax				/* restore the return value */
	jmp	_syscall64_check_for_ast	/* check again */

_syscall64_restore_state:
	/* Restore thread state and return to user using sysret. */
	cli	/* block interrupts when using the user stack in kernel space */
	movq	MY(ACTIVE_THREAD),%r11	/* point to current thread */
	movq	TH_PCB(%r11),%r11		/* point to pcb */
	addq	$ PCB_ISS,%r11			/* point to saved state */

	/* Restore syscall args. Note: we can't restore the syscall number in
	 * RAX because it needs to hold the return value.*/
	mov	R_EDI(%r11),%rdi	/* syscall arg0 */
	mov	R_ESI(%r11),%rsi	/* syscall arg1 */
	mov	R_EDX(%r11),%rdx	/* syscall arg2 */
	mov	R_R10(%r11),%r10	/* syscall arg3 */
	mov	R_R8(%r11),%r8		/* syscall arg4 */
	mov	R_R9(%r11),%r9		/* syscall arg5 */

	mov	R_UESP(%r11),%rsp	/* callee-preserved register,
					 * also switch back to user stack */
	mov	R_EIP(%r11),%rcx	/* sysret convention */
	mov	R_EBX(%r11),%rbx	/* callee-preserved register */
	mov	R_EBP(%r11),%rbp	/* callee-preserved register */
	mov	R_R12(%r11),%r12	/* callee-preserved register */
	mov	R_R13(%r11),%r13	/* callee-preserved register */
	mov	R_R14(%r11),%r14	/* callee-preserved register */
	mov	R_R15(%r11),%r15	/* callee-preserved register */
	mov	R_EFLAGS(%r11),%r11	/* sysret convention */

	swapgs
	sysretq		/* fast return to user-space, the thread didn't block */

/* Error handling fragments, from here we jump directly to the trap handler */
_syscall64_addr_push:
	movq	%r11,R_CR2(%rbx)	/* set fault address */
	movq	$(T_PAGE_FAULT),R_TRAPNO(%rbx)	/* set page-fault trap */
	movq	$(T_PF_USER),R_ERR(%rbx) /* set error code - read user space */
	movq	$RETURN_TO_USER, %r12
	jmp	_take_trap		/* treat as a trap */

_syscall64_range:
	movq	$(T_INVALID_OPCODE),R_TRAPNO(%rbx)
					/* set invalid-operation trap */
	movq	$0,R_ERR(%rbx)		/* clear error code */
	movq	$RETURN_TO_USER, %r12
	jmp	_take_trap		/* treat as a trap */

END(syscall64)

	.data
DATA(cpu_features)
DATA(cpu_features_edx)
	.long	0
DATA(cpu_features_ecx)
	.long	0
	.text

/* Discover what kind of cpu we have; return the family number
   (3, 4, 5, 6, for 386, 486, 586, 686 respectively).  */
ENTRY(discover_x86_cpu_type)
	/* We are a modern enough processor to have the CPUID instruction;
	   use it to find out what we are. */
	movl	$1,%eax			/* Fetch CPU type info ... */
	cpuid				/*  ... into eax */
	movl	%ecx,cpu_features_ecx	/* Keep a copy */
	movl	%edx,cpu_features_edx	/* Keep a copy */
	shrl	$8,%eax			/* Slide family bits down */
	andl	$15,%eax		/* And select them */
	ret				/* And return */


/**/
/*
 * Utility routines.
 */

ENTRY(copyin)
	xchgq   %rsi,%rdi		/* Get user source and kernel destination */
	movq	$VM_MAX_ADDRESS, %rcx
	cmpq	%rcx,%rsi		/* Check segment limit by hand */
	jae	copyin_fail

copyin_remainder:
	/*cld*/				/* count up: default mode in all GCC code */
	movq	%rdx,%rcx		/* move by longwords first */
	shrq	$3,%rcx
	RECOVER(copyin_fail)
	rep
	movsq				/* move longwords */
	movq	%rdx,%rcx		/* now move remaining bytes */
	andq	$7,%rcx
	RECOVER(copyin_fail)
	rep
	movsb
	xorq	%rax,%rax		/* return 0 for success */

copyin_ret:
	ret				/* and return */

copyin_fail:
	movq	$1,%rax			/* return 1 for failure */
	jmp	copyin_ret		/* pop frame and return */

bogus:
	ud2

ENTRY(copyout)
	xchgq   %rsi,%rdi		/* Get user source and kernel destination */
	movq	$VM_MAX_ADDRESS, %rcx
	cmpq	%rcx,%rdi		/* Check segment limit by hand */
	jae	copyin_fail

copyout_remainder:
	movq	%rdx,%rax		/* use count */
	/*cld*/				/* count up: always this way in GCC code */
	movq	%rax,%rcx		/* move by longwords first */
	shrq	$3,%rcx
	RECOVER(copyout_fail)
	rep
	movsq
	movq	%rax,%rcx		/* now move remaining bytes */
	andq	$7,%rcx
	RECOVER(copyout_fail)
	rep
	movsb				/* move */
	xorq	%rax,%rax		/* return 0 for success */

copyout_ret:
	ret				/* and return */

copyout_fail:
	movq	$1,%rax			/* return 1 for failure */
	jmp	copyout_ret		/* pop frame and return */

/*
 * int inst_fetch(int eip, int cs);
 *
 * Fetch instruction byte.  Return -1 if invalid address.
 */
ENTRY(inst_fetch)
	movq	S_ARG1, %rax		/* get segment */
	movw	%ax,%fs			/* into FS */
	movq	S_ARG0, %rax		/* get offset */
	movq	$VM_MAX_ADDRESS, %rcx
	cmpq	%rcx,%rax		/* Check segment limit by hand */
	jae	_inst_fetch_fault
	RETRY(EXT(inst_fetch))		/* re-load FS on retry */
	RECOVER(_inst_fetch_fault)
	movzbq	%fs:(%rax),%rax		/* load instruction byte */
	ret

_inst_fetch_fault:
	movq	$-1,%rax		/* return -1 if error */
	ret


/*
 * Done with recovery and retry tables.
 */
	RECOVER_TABLE_END
	RETRY_TABLE_END



/*
 * cpu_shutdown()
 * Force reboot
 */
null_idt:
	.space	8 * 32

null_idtr:
        .word   8 * 32 - 1
        .quad   null_idt

Entry(cpu_shutdown)
        lidt    null_idtr       /* disable the interrupt handler */
        xor     %rcx,%rcx       /* generate a divide by zero */
        div     %rcx,%rax       /* reboot now */
        ret                     /* this will "never" be executed */
