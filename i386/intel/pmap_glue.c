/* pmap_glue.c — Minimal C wrappers for macros called by Rust PMAP. */
#include <i386/proc_reg.h>
#include <i386/i386/vm_param.h>

void set_cr3_fn(unsigned long val) { set_cr3(val); }
unsigned long phystokv_fn(unsigned long pa) { return phystokv(pa); }
