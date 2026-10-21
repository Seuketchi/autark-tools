# Contributing to autark-tools

Pull requests are welcome. `main` is protected: every change goes through a pull request with passing checks and the
maintainer's review. The wider project, including adding software and the release process, is described in the
[Autark repository](https://github.com/Seuketchi/autark/blob/main/CONTRIBUTING.md).

## Before you open a pull request

```sh
cargo build --workspace --locked
cargo clippy --workspace --locked --all-targets
cargo test --workspace --locked --no-fail-fast
```

The workspace needs GTK 4 with gtk4-layer-shell 1.2 or newer, VTE, PipeWire, libcryptsetup, PAM, GStreamer, libvpx, Opus and clang.
CI builds it on Arch Linux; see `.github/workflows/ci.yml` for the exact package list.

## What review looks at

- **Risk first.** Code that runs as root or handles keys and passwords (`autark-pkg`, `autark-init`,
  `autark-hw-helper`, the Settings helper, `autark-sandbox`, `autark-lock`) gets a slower, closer review and a real test.
- **Small changes.** One thing per pull request, with the screen or command you tried it on.
- **Style.** Match the surrounding code. The workspace is not formatted with rustfmt defaults, so do not reformat files
  you are not changing.

## Security

Never commit keys, passwords or logins; CI refuses them. To report a vulnerability, do not open a public issue: use
GitHub's "Report a vulnerability" on the Security tab once it is enabled, or write to the maintainer privately.

By contributing you agree your work is licensed MIT OR Apache-2.0.
