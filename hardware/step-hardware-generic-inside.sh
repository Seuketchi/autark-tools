#!/bin/bash
# Hardware, generic: what a machine other than the target laptop needs to run on this kernel. Firmware for any
# machine, the sound DSP firmware for any Intel platform, CPU microcode for any Intel or AMD CPU, the PCI and USB
# tools (lspci, lsusb), Mesa with the NVIDIA (NVK) Vulkan driver, and a new boot image that carries the microcode.
# Runs as root (inside the chroot, or on the running system for the steps that only add files), after the
# generic kernel (ch10-kernel-inside.sh with KERNEL_PROFILE=generic) and the boot step (step6-boot-inside.sh).
# See hardware/README.md for the order, what each step costs, and how to get the offline files it reads.
#
# Choose steps with ONLY="firmware sof microcode" (default: all); a step that has finished is skipped, so delete
# /sources/.done/9-<name> to run it again.
#   FIRMWARE_SET=desktop (default)  all firmware except server network cards, datacenter switches and ARM phone chips
#   FIRMWARE_SET=full               everything linux-firmware has (about 950 MB compressed)
#   WITH_AMD=1                      also rebuild LLVM with the AMD GPU backend, and Mesa with RADV and radeonsi
set -euo pipefail

if [ "${HW_TEST:-}" != 1 ]; then
  if [ ! -e /sources/wget-list-systemd ] || grep -qi ubuntu /etc/os-release 2>/dev/null; then
    echo "step-hardware-generic-inside.sh: not inside the Autark chroot; refusing to run" >&2
    exit 1
  fi
fi

KVER=7.1.8
FW=linux-firmware-20260916
SOF=sof-bin-2026.09.2
UCODE=Intel-Linux-Processor-Microcode-Data-Files-microcode-20260925
FIRMWARE_SET=${FIRMWARE_SET:-desktop}
WITH_AMD=${WITH_AMD:-0}
OFFLINE=${OFFLINE:-/sources/hw-offline}
FWDIR=${FWDIR:-/usr/lib/firmware}
BOOT=${BOOT:-/boot}
only=" ${ONLY:-} "

T_LIBUSB=libusb-1.0.29.tar.bz2
T_PCIUTILS=pciutils-3.15.0.tar.xz
T_USBUTILS=usbutils-019.tar.xz
T_MESA=mesa-26.1.7.tar.xz
T_LLVM=llvm-project-22.1.8.src.tar.xz
P_LLVM=llvm-22.1.8-upstream_fix-1.patch

# Firmware that no x86 laptop or PC takes: server network cards (Marvell Prestera switches, Mellanox, QLogic, Netronome,
# NXP DPAA2), Qualcomm phone and laptop SoCs. `full` keeps them.
FIRMWARE_SKIP="qcom mellanox qed netronome dpaa2 mrvl/prestera"

if [ "${HW_TEST:-}" != 1 ]; then
  logs=/sources/logs
  mkdir -p "$logs" /sources/.done /sources/.track /var/lib/autark/files
  srcdir=/sources
  source "$(dirname "$0")/build-lib.sh"

  export CFLAGS="-O2 -march=x86-64-v3 -pipe -fstack-protector-strong -fstack-clash-protection -fcf-protection=full -D_FORTIFY_SOURCE=3"
  export CXXFLAGS="$CFLAGS"
  export LDFLAGS="-Wl,-O1 -Wl,-z,relro -Wl,-z,now"

  # The files each step adds, for the package tracking (as the other step scripts do).
  snapshot() {
    find / -xdev \( -path /sources -o -path /tmp -o -path /root -o -path /var/lib/autark \) -prune \
      -o \( -type f -o -type l \) -print | LC_ALL=C sort
  }

  run_pkg() {
    local name=$1 tarball=$2 fn=$3
    if [ "$only" != "  " ]; then
      case "$only" in *" $name "*) ;; *) return ;; esac
    fi
    if [ -e "/sources/.done/9-$name" ]; then echo "== $name: already done, skipped"; return; fi
    snapshot > /sources/.track/before
    if [ "$tarball" = - ]; then
      echo "== $fn: started $(date +%T)"
      set +e; ( set -e; cd /sources; "$fn" ) > "$logs/$fn.log" 2>&1; local rc=$?; set -e
      if [ "$rc" != 0 ]; then echo "== $fn: FAILED (exit $rc)"; tail -25 "$logs/$fn.log"; exit 1; fi
      echo "== $fn: done"
    else
      build "$tarball" "$fn"
    fi
    snapshot | LC_ALL=C comm -13 /sources/.track/before - > "/var/lib/autark/files/9-$name"
    echo "   $(wc -l < "/var/lib/autark/files/9-$name") new files tracked"
    touch "/sources/.done/9-$name"
  }
fi

# ---- Firmware ----

# Every firmware file linux-firmware has, compressed with zstd (the kernel reads them as they are: FW_LOADER_COMPRESS_ZSTD),
# less what FIRMWARE_SKIP lists. Its own installer lays the files out and makes the old-name links its WHENCE file defines
# for Wi-Fi; links into a folder that was left out are removed, not left to dangle. CPU microcode stays plain, as the
# early loader can't read it compressed.
step_firmware_generic() {
  local t stage skip
  t=$(mktemp -d)
  tar -xJf /sources/$FW.tar.xz -C "$t"
  stage=$t/stage
  ( cd "$t/$FW" && ./copy-firmware.sh --zstd "$stage" )
  if [ "$FIRMWARE_SET" != full ]; then
    for skip in $FIRMWARE_SKIP; do rm -rf "${stage:?}/$skip"; done
  fi
  find "$stage" -xtype l -delete
  install -d "$FWDIR"
  cp -a "$stage"/. "$FWDIR"/
  echo "== firmware ($FIRMWARE_SET): $(find "$stage" \( -type f -o -type l \) | wc -l) files, $(du -sh "$stage" | cut -f1)"
  rm -rf "$t"
}

# The sound DSP firmware of Intel platforms (not in linux-firmware): the whole package as it lays itself out, without the
# community builds, which a retail DSP refuses (only Intel-signed firmware runs on one). The links in it point into the
# intel-signed folders; those whose target is gone are removed.
step_sof_generic() {
  local t src dest
  t=$(mktemp -d)
  tar -xzf /sources/$SOF.tar.gz -C "$t"
  src=$t/$SOF
  dest=$FWDIR/intel
  install -d "$dest"
  for d in sof sof-ipc4 sof-ipc4-lib sof-tplg sof-ipc4-tplg sof-ace-tplg; do
    [ -d "$src/$d" ] || continue
    install -d "$dest/$d"
    cp -a "$src/$d"/. "$dest/$d"/
  done
  find "$dest" -type d -name community -prune -exec rm -rf {} +
  find "$dest" -xtype l -delete
  echo "== sound firmware: $(find "$dest" -path '*sof*' \( -type f -o -type l \) | wc -l) files"
  rm -rf "$t"
}

# Microcode for any CPU, not just the target's: every Intel file in one early image, and AMD's from linux-firmware, in
# /boot/microcode.img, which the boot image puts first. The kernel picks the file for the CPU it's on.
step_microcode_generic() {
  local t
  t=$(mktemp -d)
  tar -xzf /sources/intel-microcode-20260925.tar.gz -C "$t" "$UCODE/intel-ucode"
  install -d "$FWDIR/intel-ucode" "$FWDIR/amd-ucode"
  find "$t/$UCODE/intel-ucode" -maxdepth 1 -type f -exec install -vm644 {} "$FWDIR/intel-ucode/" \;
  tar -xJf /sources/$FW.tar.xz -C "$t" --wildcards "$FW/amd-ucode/*.bin"
  install -vm644 "$t/$FW"/amd-ucode/*.bin "$FWDIR/amd-ucode/"
  mkdir -p "$t/early/kernel/x86/microcode"
  find "$t/$UCODE/intel-ucode" -maxdepth 1 -type f | LC_ALL=C sort | xargs cat > "$t/early/kernel/x86/microcode/GenuineIntel.bin"
  find "$t"/$FW/amd-ucode -maxdepth 1 -name '*.bin' | LC_ALL=C sort | xargs cat > "$t/early/kernel/x86/microcode/AuthenticAMD.bin"
  ( cd "$t/early" && find . | cpio -o -H newc --reproducible ) > "$BOOT/microcode.img"
  echo "== microcode image: $(du -h "$BOOT/microcode.img" | cut -f1) (Intel $(ls "$FWDIR/intel-ucode" | wc -l) files, AMD $(ls "$FWDIR/amd-ucode" | wc -l))"
  rm -rf "$t"
}

# ---- Tools: lspci, setpci, lsusb ----

pkg_libusb() {
  ./configure --prefix=/usr --disable-static --disable-udev
  make
  make install
}

# Not `make install`: that also installs pciutils' own pci.ids over hwdata's newer one. The IDs come from hwdata.
pkg_pciutils() {
  make PREFIX=/usr SBINDIR=/usr/bin SHARED=yes ZLIB=no DNS=no IDSDIR=/usr/share/hwdata
  make PREFIX=/usr SBINDIR=/usr/bin SHARED=yes ZLIB=no DNS=no IDSDIR=/usr/share/hwdata install-lib
  install -vm755 lspci setpci /usr/bin/
  install -vDm644 lspci.8 /usr/share/man/man8/lspci.8
  install -vDm644 setpci.8 /usr/share/man/man8/setpci.8
}

pkg_usbutils() {
  mkdir build
  cd    build
  meson setup .. --prefix=/usr --buildtype=release
  ninja
  ninja install
}

# ---- Mesa with the NVIDIA Vulkan driver (NVK) ----

# NVK is written in Rust and needs two code generators, bindgen and cbindgen. Built from the crates that
# hardware/prepare-offline.sh vendored into $OFFLINE/crates-vendor, with no network, as the chroot has none.
step_rust_tools() {
  local cfg
  cfg=$(mktemp -d)
  mkdir -p "$cfg/.cargo"
  printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "%s/crates-vendor"\n' "$OFFLINE" > "$cfg/.cargo/config.toml"
  ( cd "$cfg" && cargo install bindgen-cli cbindgen --offline --root /usr )
  bindgen --version
  cbindgen --version
  rm -rf "$cfg"
}

# The same as phase 8a's Mesa, plus NVK and zink (OpenGL on top of Vulkan, which is how NVIDIA GPUs get OpenGL with
# NVK). Mesa's Rust code needs six crates it would download; they come from the cache prepare-offline.sh made.
# AMD GPUs need LLVM's AMDGPU backend for both RADV and radeonsi (Mesa asks for it whichever way it's configured),
# so they're only built when WITH_AMD=1, after LLVM has been rebuilt with it.
pkg_mesa_generic() {
  local vulkan=intel,nouveau,swrast gallium=iris,llvmpipe,zink
  if [ "$WITH_AMD" = 1 ]; then
    vulkan=$vulkan,amd
    gallium=$gallium,radeonsi
  fi
  mkdir -p subprojects/packagecache
  cp "$OFFLINE"/mesa-packagecache/* subprojects/packagecache/
  mkdir build
  cd    build
  meson setup ..                     \
        --prefix=/usr                \
        --buildtype=release          \
        --wrap-mode=nodownload       \
        -D platforms=wayland         \
        -D gallium-drivers=$gallium  \
        -D vulkan-drivers=$vulkan    \
        -D llvm=enabled              \
        -D glx=disabled              \
        -D valgrind=disabled         \
        -D libunwind=disabled
  ninja
  ninja install
}

# Phase 8a's LLVM, recipe for recipe, with the AMD GPU backend added to the targets: a long build, and the only way
# to get AMD GPUs (phase 8a narrowed LLVM to the host's target to save the build, on purpose).
pkg_llvm_amd() {
  patch -Np1 -i "$srcdir/$P_LLVM"
  grep -rl '#!.*python$' | xargs sed -i '1s/python$/python3/'
  mkdir -pv /etc/clang
  for i in clang clang++; do
    echo -fstack-protector-strong > /etc/clang/$i.cfg
  done

  mkdir -v llvm/build
  cd       llvm/build
  CC=gcc CXX=g++                               \
  cmake -D CMAKE_INSTALL_PREFIX=/usr           \
        -D CMAKE_SKIP_INSTALL_RPATH=ON         \
        -D LLVM_ENABLE_FFI=ON                  \
        -D CMAKE_BUILD_TYPE=Release            \
        -D LLVM_BUILD_LLVM_DYLIB=ON            \
        -D LLVM_LINK_LLVM_DYLIB=ON             \
        -D LLVM_ENABLE_RTTI=ON                 \
        -D LLVM_TARGETS_TO_BUILD="host;AMDGPU" \
        -D LLVM_ENABLE_PROJECTS=clang          \
        -D LLVM_BINUTILS_INCDIR=/usr/include   \
        -D LLVM_INCLUDE_BENCHMARKS=OFF         \
        -D LLVM_INCLUDE_TESTS=OFF              \
        -D LLVM_PARALLEL_LINK_JOBS=2           \
        -D CLANG_DEFAULT_PIE_ON_LINUX=ON       \
        -D CLANG_CONFIG_FILE_SYSTEM_DIR=/etc/clang \
        -W no-author -G Ninja ..
  ninja
  ninja install
}

# ---- The boot image ----

# Rebuilds the initramfs and the unified kernel image, so the new microcode is in the image that boots. step_uki is the
# boot step's own, sourced from step6-boot-inside.sh, so the image is made and signed exactly as it always is. That file
# runs ALL of its steps when sourced unless step6-sourceable.patch is applied, so this refuses to go on without it.
step_boot_image() {
  local boot=$srcdir/step6-boot-inside.sh
  grep -q AUTARK_SOURCE_ONLY "$boot" || { echo "step_boot_image: apply step6-sourceable.patch to $boot first" >&2; exit 1; }
  AUTARK_SOURCE_ONLY=1 source "$boot"
  step_initramfs
  step_uki
}

if [ "${HW_TEST:-}" != 1 ]; then
  run_pkg firmware   -                  step_firmware_generic
  run_pkg sof        -                  step_sof_generic
  run_pkg microcode  -                  step_microcode_generic
  run_pkg libusb     "$T_LIBUSB"        pkg_libusb
  run_pkg pciutils   "$T_PCIUTILS"      pkg_pciutils
  run_pkg usbutils   "$T_USBUTILS"      pkg_usbutils
  run_pkg rust-tools -                  step_rust_tools
  if [ "$WITH_AMD" = 1 ]; then
    run_pkg llvm-amd "$T_LLVM"          pkg_llvm_amd
  fi
  run_pkg mesa       "$T_MESA"          pkg_mesa_generic
  run_pkg boot-image -                  step_boot_image
  echo "== generic hardware support done: reboot to use the new boot image"
fi
