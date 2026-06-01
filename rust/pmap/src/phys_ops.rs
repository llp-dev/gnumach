// rust/pmap/src/phys_ops.rs — Physical memory operations.
use crate::types::*;

pub fn kvtophys(va: VmOffset) -> PhysAddr {
    extern "C" { fn kvtophys(va: VmOffset) -> PhysAddr; }
    unsafe { kvtophys(va) }
}

pub fn copy_to_phys(src: VmOffset, dst: PhysAddr, count: i32) {
    extern "C" { fn copy_to_phys(src: VmOffset, dst: PhysAddr, count: i32); }
    unsafe { copy_to_phys(src, dst, count); }
}

pub fn copy_from_phys(src: PhysAddr, dst: VmOffset, count: i32) {
    extern "C" { fn copy_from_phys(src: PhysAddr, dst: VmOffset, count: i32); }
    unsafe { copy_from_phys(src, dst, count); }
}
