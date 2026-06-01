// rust/pmap/src/smp.rs — Multi-CPU TLB shootdown (stub, delegates to C).
use crate::types::*;
use core::ffi::c_int;

pub fn signal_cpus(_use_list: CpuSet, _pmap: *mut Pmap, _start: VmOffset, _end: VmOffset) {
    extern "C" { fn signal_cpus_c(use_list: CpuSet, pmap: *mut Pmap, start: VmOffset, end: VmOffset); }
    unsafe { signal_cpus_c(_use_list, _pmap, _start, _end); }
}

pub fn process_pmap_updates(_my_pmap: *mut Pmap) {
    extern "C" { fn process_pmap_updates_c(pmap: *mut Pmap); }
    unsafe { process_pmap_updates_c(_my_pmap); }
}

pub fn pmap_update_interrupt() {
    extern "C" { fn pmap_update_interrupt_c(); }
    unsafe { pmap_update_interrupt_c(); }
}
