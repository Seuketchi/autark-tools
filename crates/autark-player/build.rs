//! Compiles the small C layer over GStreamer's playbin and appsink that `autark-player` plays through.

fn main() {
    let mut build = cc::Build::new();
    let mut libs = Vec::new();
    let mut paths = Vec::new();
    for name in ["gstreamer-1.0", "gstreamer-app-1.0", "gstreamer-video-1.0"] {
        // Not printed yet: the libraries must come after the C layer on the linker's line, or `--as-needed` drops them.
        let library = pkg_config::Config::new().cargo_metadata(false).probe(name).unwrap_or_else(|err| panic!("{name}: {err}"));
        build.includes(library.include_paths);
        libs.extend(library.libs);
        paths.extend(library.link_paths);
    }
    build.file("csrc/play_shim.c").warnings(true).compile("play_shim");
    for path in paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for lib in libs {
        println!("cargo:rustc-link-lib={lib}");
    }
    println!("cargo:rerun-if-changed=csrc/play_shim.c");
}
