// rust/pmap/src/activation.rs — Address space switching.
use crate::types::*;

pub fn pmap_activate(pmap: *mut Pmap, _thread: Thread, _cpu: c_int) {
    if pmap.is_null() { return; }
    #[cfg(feature = "smp")]
    {
        extern "C" { fn pmap_activate_smp(pmap: *mut Pmap, cpu: c_int); }
        unsafe { pmap_activate_smp(pmap, _cpu); }
    }
    #[cfg(not(feature = "smp"))]
    {
        unsafe { set_pmap(&*pmap); (*pmap).cpus_using = 1; }
    }
}

pub fn pmap_deactivate(pmap: *mut Pmap, _thread: Thread, _cpu: c_int) {
    if pmap.is_null() { return; }
    unsafe {
        #[cfg(feature = "smp")]
        { extern "C" { fn i_bit_clear(bit: c_int, set: *mut CpuSet); } i_bit_clear(_cpu, &mut (*pmap).cpus_using); }
        #[cfg(not(feature = "smp"))]
        { (*pmap).cpus_using = 0; }
    }
}

unsafe fn set_pmap(pmap: &Pmap) {
    extern "C" { fn set_cr3_fn(value: PhysAddr); fn kvtophys(va: VmOffset) -> PhysAddr; }
    #[cfg(feature = "x86_64")] { set_cr3_fn(kvtophys((*pmap).l4base as VmOffset)); }
    #[cfg(all(feature = "pae", not(feature = "x86_64")))] { set_cr3_fn(kvtophys((*pmap).pdpbase as VmOffset)); }
    #[cfg(not(feature = "pae"))] { set_cr3_fn(kvtophys((*pmap).dirbase as VmOffset)); }
}

use core::ffi::c_int;
