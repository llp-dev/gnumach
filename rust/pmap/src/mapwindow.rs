// rust/pmap/src/mapwindow.rs — Temporary kernel mapping windows.
use crate::types::*;

pub fn pmap_get_mapwindow(entry: PhysAddr) -> *mut PmapMapwindow {
    extern "C" { fn pmap_get_mapwindow(entry: PhysAddr) -> *mut PmapMapwindow; }
    unsafe { pmap_get_mapwindow(entry) }
}

pub fn pmap_put_mapwindow(map: *mut PmapMapwindow) {
    extern "C" { fn pmap_put_mapwindow(map: *mut PmapMapwindow); }
    unsafe { pmap_put_mapwindow(map); }
}
