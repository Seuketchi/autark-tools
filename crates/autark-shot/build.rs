//! Compiles the small C layer over libvpx that `autark-shot record` encodes with, and links libvpx and libopus.

fn main() {
    cc::Build::new().file("csrc/vpx_shim.c").warnings(true).compile("vpx_shim");
    println!("cargo:rustc-link-lib=vpx");
    println!("cargo:rustc-link-lib=opus");
    println!("cargo:rerun-if-changed=csrc/vpx_shim.c");
}
