#!/bin/bash
# Fetches what step-hardware-generic-inside.sh needs and the chroot can't download: three source tarballs (checked against
# the hashes below), the crates for bindgen and cbindgen (vendored, 85 MB), and the six small Rust crates Mesa's NVK
# driver would download while building. Run it on the host, as yourself, with a network; /sources is world-writable.
#
#   SOURCES=/sources ./prepare-offline.sh
set -euo pipefail

SOURCES=${SOURCES:-/sources}
OFFLINE=${OFFLINE:-$SOURCES/hw-offline}
MESA=mesa-26.1.7
mkdir -p "$OFFLINE"

fetch() {  # url file sha256
  local url=$1 file=$2 sum=$3
  if [ ! -e "$SOURCES/$file" ]; then
    curl -fL --retry 3 -o "$SOURCES/$file.part" "$url"
    mv "$SOURCES/$file.part" "$SOURCES/$file"
  fi
  echo "$sum  $SOURCES/$file" | sha256sum -c -
}

# pciutils and usbutils: the hashes are in kernel.org's sha256sums.asc beside each tarball.
fetch https://mirrors.edge.kernel.org/pub/software/utils/pciutils/pciutils-3.15.0.tar.xz pciutils-3.15.0.tar.xz \
      c02940f430841ecf158d5d9a50007afc4d5353c8678a2455003ca0b2c4e9f5ff
fetch https://mirrors.edge.kernel.org/pub/linux/utils/usb/usbutils/usbutils-019.tar.xz usbutils-019.tar.xz \
      659f40c440e31ba865c52c818a33d3ba6a97349e3353f8b1985179cb2aa71ec5
# libusb: the official release asset; GitHub publishes no hash for it, so this one was taken from the download itself.
fetch https://github.com/libusb/libusb/releases/download/v1.0.29/libusb-1.0.29.tar.bz2 libusb-1.0.29.tar.bz2 \
      5977fc950f8d1395ccea9bd48c06b3f808fd3c2c961b44b0c2e6e29fc3a70a85

# bindgen and cbindgen, with every crate they need, so cargo builds them with no network.
if [ ! -d "$OFFLINE/crates-vendor" ]; then
  tmp=$(mktemp -d)
  mkdir -p "$tmp/src"
  cat > "$tmp/Cargo.toml" << 'TOML'
[package]
name = "hw-tools-vendor"
version = "0.0.0"
edition = "2021"

[dependencies]
bindgen-cli = "=0.73.2"
cbindgen = "=0.29.4"
TOML
  echo 'fn main() {}' > "$tmp/src/main.rs"
  ( cd "$tmp" && cargo vendor "$OFFLINE/crates-vendor" > /dev/null )
  rm -rf "$tmp"
fi

# Mesa's NVK driver is Rust and meson would fetch these while configuring. They're the whole list: paste, proc-macro2,
# quote, rustc-hash, syn and unicode-ident (428 KB). Configuring a fresh tree with only these, with downloads off,
# succeeds (checked for Mesa 26.1.7).
if [ ! -d "$OFFLINE/mesa-packagecache" ]; then
  tmp=$(mktemp -d)
  tar -xf "$SOURCES/$MESA.tar.xz" -C "$tmp"
  ( cd "$tmp/$MESA" && meson subprojects download paste-1-rs proc-macro2-1-rs quote-1-rs rustc-hash-2-rs syn-2-rs unicode-ident-1-rs > /dev/null )
  mkdir -p "$OFFLINE/mesa-packagecache"
  cp "$tmp/$MESA"/subprojects/packagecache/* "$OFFLINE/mesa-packagecache/"
  rm -rf "$tmp"
fi

echo "== offline files ready in $OFFLINE:"
du -sh "$OFFLINE"/* | sed 's/^/   /'
ls "$OFFLINE/mesa-packagecache"
