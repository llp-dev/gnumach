/*
 * pmap_bootstrap.c — Bootstrap functions extracted from pmap.c.
 *
 * This file retains the functions that run before or during early
 * kernel initialization (before the Rust runtime can execute).
 * The remaining PMAP functions are implemented in rust/pmap/src/.
 *
 * Functions retained:
 *   pmap_bootstrap() — Main bootstrap, builds initial page tables
 *   pmap_bootstrap_pae() — PAE-specific bootstrap
 *   pmap_bootstrap_xen() — Xen PV bootstrap
 *   pmap_set_page_dir() — Sets CR3/CR4
 *   pmap_unmap_page_zero() — Unmaps page 0
 *   pmap_make_temporary_mapping() — Temporary mapping for GDT transition
 *   pmap_remove_temporary_mapping() — Removes temporary mapping
 *   pmap_clear_bootstrap_pagetable() — Xen bootstrap cleanup
 *   pmap_init() — Post-VM-initialization setup (slab caches, pv_head_table)
 *
 * Global data initialized here:
 *   kernel_pmap, kernel_page_dir, kernel_virtual_start/end,
 *   pv_lock_table, pv_head_table, pmap_phys_attributes,
 *   pmap_initialized, pmap_object, pmap_cache, pdpt_cache,
 *   pv_list_cache, pmap_system_lock, cpus_active, cpus_idle,
 *   cpu_update_needed
 */

#include <string.h>

#include <mach/machine/vm_types.h>
#include <mach/boolean.h>
#include <kern/debug.h>
#include <kern/printf.h>
#include <kern/thread.h>
#include <kern/slab.h>
#include <kern/lock.h>

#include <vm/pmap.h>
#include <vm/vm_map.h>
#include <vm/vm_kern.h>
#include <i386/vm_param.h>
#include <mach/vm_prot.h>
#include <vm/vm_object.h>
#include <vm/vm_page.h>
#include <vm/vm_user.h>

#include <mach/machine/vm_param.h>
#include <mach/xen.h>
#include <machine/thread.h>
#include <i386/cpu_number.h>
#include <i386/proc_reg.h>
#include <i386/locore.h>
#include <i386/model_dep.h>
#include <i386/spl.h>
#include <i386at/biosmem.h>
#include <i386at/model_dep.h>

#if NCPUS > 1
#include <i386/mp_desc.h>
#endif

#include <ddb/db_output.h>
#include <machine/db_machdep.h>

/* ─────────────────────────────────────────────────────────────────
 * Global state (accessed by both C bootstrap and Rust PMAP)
 * ───────────────────────────────────────────────────────────────── */

pmap_t          kernel_pmap;           /* the kernel's address space */
pt_entry_t     *kernel_page_dir;       /* kernel page directory base */
vm_offset_t     kernel_virtual_start;  /* kernel VM range start */
vm_offset_t     kernel_virtual_end;    /* kernel VM range end */

char           *pv_lock_table;         /* lock bits for pv_head entries */
struct pv_entry *pv_head_table;        /* array of pv_entry, one per vm_page */
char           *pmap_phys_attributes;  /* modified/reference attribute bytes */

boolean_t       pmap_initialized = FALSE; /* set to TRUE after pmap_init() */

vm_object_t     pmap_object = VM_OBJECT_NULL; /* physical memory object */

/* Slab caches */
struct kmem_cache   pmap_cache;       /* pmap structs */
struct kmem_cache   pdpt_cache;       /* page directory pointer tables */
struct kmem_cache   pv_list_cache;    /* pv_entry structures */

/* Lock on pv free list */
decl_simple_lock_data(static, pv_free_list_lock)
struct pv_entry *pv_free_list;

#if NCPUS > 1
/* Multi-CPU state */
lock_data_t         pmap_system_lock;
cpu_set             cpus_active;
cpu_set             cpus_idle;
boolean_t           cpu_update_needed[NCPUS];
#endif

/* PMAP map windows (per-CPU) */
pmap_mapwindow_t    pmap_mapwindow[PMAP_NMAPWINDOWS];

/* ─────────────────────────────────────────────────────────────────
 * Bootstrap functions — extracted from pmap.c
 * ───────────────────────────────────────────────────────────────── */

/*
 * For the initial migration, the bootstrap functions below are stubs
 * that call through to the original implementations in pmap.c.
 * After full validation, they will be replaced with extracted copies.
 */

static int bootstrap_initialized = 0;

void pmap_bootstrap(void)
{
    /*
     * The original pmap_bootstrap() is in pmap.c (line ~736).
     * This function must run with paging off.
     * For now, we declare an extern reference to the original.
     * When pmap.c is removed, this function will contain the full
     * extracted bootstrap code.
     */
    extern void pmap_bootstrap_original(void);
    if (!bootstrap_initialized) {
        bootstrap_initialized = 1;
        pmap_bootstrap_original();
    }
}

void pmap_set_page_dir(void)
{
    extern void pmap_set_page_dir_original(void);
    pmap_set_page_dir_original();
}

void pmap_unmap_page_zero(void)
{
    extern void pmap_unmap_page_zero_original(void);
    pmap_unmap_page_zero_original();
}

void pmap_make_temporary_mapping(void)
{
    extern void pmap_make_temporary_mapping_original(void);
    pmap_make_temporary_mapping_original();
}

void pmap_remove_temporary_mapping(void)
{
    extern void pmap_remove_temporary_mapping_original(void);
    pmap_remove_temporary_mapping_original();
}

void pmap_init(void)
{
    extern void pmap_init_original(void);
    pmap_init_original();
}

#ifdef MACH_PV_PAGETABLES
void pmap_clear_bootstrap_pagetable(pt_entry_t *addr)
{
    extern void pmap_clear_bootstrap_pagetable_original(pt_entry_t *);
    pmap_clear_bootstrap_pagetable_original(addr);
}
#endif /* MACH_PV_PAGETABLES */

#ifdef PAE
void pmap_bootstrap_pae(void)
{
    extern void pmap_bootstrap_pae_original(void);
    pmap_bootstrap_pae_original();
}
#endif /* PAE */

#ifdef MACH_PV_PAGETABLES
void pmap_bootstrap_xen(pt_entry_t *l1_map[])
{
    extern void pmap_bootstrap_xen_original(pt_entry_t *[]);
    pmap_bootstrap_xen_original(l1_map);
}
#endif /* MACH_PV_PAGETABLES */
