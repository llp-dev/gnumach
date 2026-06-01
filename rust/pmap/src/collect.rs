// rust/pmap/src/collect.rs — Page table garbage collection.
use crate::types::*;
pub fn pmap_collect(_p: *mut Pmap) { crate::lifecycle::pmap_collect(_p); }
