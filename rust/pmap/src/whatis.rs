// rust/pmap/src/whatis.rs — Kernel debugger address inspection (stub).
use crate::types::*;
use core::ffi::c_int;

pub fn pmap_whatis(_pmap: *mut Pmap, _a: VmOffset) -> c_int {
    extern "C" { fn pmap_whatis_c(pmap: *mut Pmap, a: VmOffset) -> c_int; }
    unsafe { pmap_whatis_c(_pmap, _a) }
}
