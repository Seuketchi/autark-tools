This crate is written but deliberately not listed in the workspace `members` in ../../Cargo.toml yet.

It needs `vte4` (the Rust bindings for VTE, GTK4 flavour), which is not in the vendored crates the workspace builds against
(/sources/crates-8b), and VTE 0.84 itself must be installed on the system first (phase8d: fmt, simdutf, vte). Listing it earlier made
`cargo build` fail for the whole workspace.

To enable it: install VTE (phase8d), vendor `vte4 0.10.0`, `vte4-sys 0.10.0` and `io-lifetimes 2.0.4` next to the other crates
(a merged copy is in ~/crates-merged), then add "crates/autark-terminal" back to `members`.
