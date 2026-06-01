// rust/pmap/build.rs — Bindgen FFI type generation
use std::env;
use std::path::PathBuf;

fn main() {
    let kernel_include = PathBuf::from("..").join("include");
    let i386_include = PathBuf::from("..").join("i386").join("include");

    if !kernel_include.exists() {
        eprintln!("cargo:warning=kernel headers not found, skipping bindgen");
        return;
    }

    let bindings = bindgen::Builder::default()
        .header("../i386/intel/pmap.h")
        .header("../vm/pmap.h")
        .header("../include/mach/vm_prot.h")
        .header("../include/mach/vm_statistics.h")
        .header("../include/mach/kern_return.h")
        .clang_args(&[
            "-I", "../i386/include",
            "-I", "../include",
            "-I", "..",
            "-I", "../i386",
            "-DMACH_KERNEL",
            "-D__ELF__",
        ])
        .use_core()
        .ctypes_prefix("crate::ctypes")
        .derive_default(true)
        .derive_eq(true)
        .derive_ord(true)
        .generate_comments(false)
        .layout_tests(false)
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings");
}
