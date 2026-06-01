#![no_std]
#![cfg_attr(not(any(test, feature = "host-test")), no_main)]

#[cfg(not(any(test, feature = "host-test")))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

mod types;
mod locking;

#[cfg(not(any(test, feature = "host-test")))]
mod lifecycle;
#[cfg(not(any(test, feature = "host-test")))]
mod mapping;
#[cfg(not(any(test, feature = "host-test")))]
mod protection;
#[cfg(not(any(test, feature = "host-test")))]
mod pv_list;
#[cfg(not(any(test, feature = "host-test")))]
mod expand;
#[cfg(not(any(test, feature = "host-test")))]
mod page_attr;
#[cfg(not(any(test, feature = "host-test")))]
mod phys_ops;
#[cfg(not(any(test, feature = "host-test")))]
mod mapwindow;
#[cfg(not(any(test, feature = "host-test")))]
mod activation;
#[cfg(not(any(test, feature = "host-test")))]
mod collect;

#[cfg(all(not(any(test, feature = "host-test")), feature = "kdb"))]
mod whatis;
#[cfg(all(not(any(test, feature = "host-test")), feature = "smp"))]
mod smp;
#[cfg(all(not(any(test, feature = "host-test")), feature = "xen"))]
mod xen;

#[cfg(not(any(test, feature = "host-test")))]
mod ffi;
