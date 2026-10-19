# Hardware: generic, dynamic, standard

Autark's drivers should come from the kernel's own mechanisms, not a table of known machines: the kernel carries every
driver it can build, udev loads the right one from a device's ID, firmware is in `/lib/firmware`, and what a laptop
offers (lights, fans, battery limits) is read from the places the kernel puts it. A machine nobody has seen is then
described as well as one everybody has. This folder is that, in three parts.

## What was found on this laptop (an MSI Cyborg 15 A13UCK)

All of this was read from the machine, not assumed.

| Finding | Consequence |
| --- | --- |
| The kernel is **trimmed to this laptop**: `ch10-kernel-inside.sh` runs `localmodconfig` from `target-lsmod.txt`, so it has 285 modules where the distribution config it starts from (`target-kernel.config`) has 6,709. | Any other machine finds no driver for most of its hardware. |
| Firmware is "exactly this laptop's": about 20 files, 21 MB (`step6-boot-inside.sh`). CPU microcode is the one file for this CPU. | Other machines' Wi-Fi, GPUs and CPUs can't load theirs. |
| Mesa builds Intel graphics only (`gallium-drivers=iris,llvmpipe`, `vulkan-drivers=intel,swrast`), and LLVM only for the host target. | No NVIDIA or AMD graphics on any machine. |
| The **NVIDIA RTX 3050** (`10de:25a2`) has no driver bound. nouveau is already built as a module in the base config; the kernel was just trimmed past it. The GSP firmware it needs (`nvidia/ga102/gsp/gsp-570.144.bin`, shared by GA107) is in the firmware tarball. | Generic kernel + firmware + Mesa NVK gives it a driver, with no new kernel options. |
| The kernel's `msi_ec` driver (fans, keyboard light, battery limit) refuses this laptop: *Firmware version is not supported: '15K1IMS1.113'*. No `15K1` entry exists in 7.1.8. | The keyboard light, fan modes and charge limit aren't offered by the kernel here. |
| The keyboard's lights are a USB device, `MysticLight MS-1565` (`1462:1601`), with a root-only `/dev/hidraw0`. | No standard interface; needs a driver profile, and a udev rule to let the user talk to it. |
| Modules must be **signed** (`MODULE_SIG_FORCE`), with a key made fresh by each kernel build and kept in `/root/kernel-keys`. | New or out-of-tree modules need that key; a kernel rebuild signs everything it builds. |
| `lspci` and `lsusb` aren't installed. (`modprobe`, `lsmod` and `depmod` are, in `/usr/sbin`.) | Added below. |
| `/home` is nearly full (8.8 GB disk, 95% used). | See "Disk" at the end. |

## What to type, in order

Everything that doesn't need root has been done: the binaries are built (`target/release/autark-hw` and
`autark-hw-helper`), and `prepare-offline.sh` has been run, so `/sources/hw-offline` and the three tarballs are in place.
Each line below needs your password, so run it yourself (in Claude Code, prefix with `!`):

1. **The helper and the Hardware app, into `/usr`** (safe, only adds files):
   `sudo sh ~/autark-tools/hardware/install-helper.sh`
2. **Firmware, sound firmware and `lspci`/`lsusb`** (safe on the running system, only adds files):
   `sudo cp ~/autark-tools/hardware/step-hardware-generic-inside.sh /sources/ && sudo env ONLY="firmware sof libusb pciutils usbutils" bash /sources/step-hardware-generic-inside.sh`
3. **Later, in the maintenance flow** (they replace the kernel, Mesa or the boot image, so reboot after): the generic
   kernel (section 3A), then `microcode`, `rust-tools`, `mesa` and `boot-image` from the same script.
   `boot-image` needs `step6-sourceable.patch` applied first, and refuses to run without it.

If the workspace stops resolving, `.cargo/config.toml` has a `[patch.crates-io]` block pointing at `vendor-extra/` (three
crates `autark-terminal` needs: `vte4`, `vte4-sys`, `io-lifetimes`). Delete both once they're in `/sources/crates-8b`.

## 1. What's built and working now (no root needed)

`autark-hw`, in `crates/autark-hw`:

- `autark-hw devices` lists every PCI and USB device with its driver; `autark-hw doctor` lists only what needs attention and
  says what to do; `autark-hw controls` shows the laptop's backlights, battery, fans, radios and power profile.
  On this laptop `doctor` reports exactly the two real problems above.
- `autark-hw` with no arguments opens the **Hardware** window: what needs attention, the laptop's controls (screen
  brightness, battery, cooling and temperatures, radios, and the keyboard backlight, power profile and charge limit where
  the kernel offers them), and every device.
- `autark-hw watch` stays running and shows a notification when hardware that needs attention appears: a USB device
  with no driver, a card whose driver exists but isn't loaded, firmware a driver can't find. It looks again a couple of
  seconds after the last event, so one plug-in isn't announced once per interface.

How it knows: udev's database (names, bound drivers, modaliases), `modules.alias` (which module claims which device),
`/proc/modules`, `journalctl -k` (firmware that failed to load; drivers that don't know a model), and sysfs for controls.
Controls the kernel doesn't offer are shown as exactly that, never faked.

## 2. The root helper (written and tested; installing it needs root once)

`crates/autark-hw-helper` is a small D-Bus service (`org.autark.Hardware1`, started on demand, exits when idle, in a
tight sandbox with only `CAP_SYS_MODULE`). It does two things, each checked against a short list before polkit is asked:

- `WriteSetting`: the power profile, a battery's charge limit and start threshold, and charging behaviour (auto or
  inhibit only). Paths are rebuilt from checked parts (no `..`, no chargers, only batteries), and values must be ones the
  setting takes (a charge limit is 20 to 100).
- `LoadModule`: loads a module only if it drives hardware that is actually here and isn't loaded yet.

Polkit lets whoever is at the machine do both; anyone else must be an administrator.
Install it with `su -c 'sh /home/tan/autark-tools/hardware/install-helper.sh'` (needs the binaries built). Until then
the Hardware window still works and says "the hardware helper isn't installed" if you try something your own user
can't do.

## 3. Making it generic (needs root and long builds)

Everything here was prepared and checked as far as it can be without doing the build. The order matters.

**Get the offline files first** (as yourself, with a network): `./prepare-offline.sh`. It downloads and checks
libusb, pciutils and usbutils, vendors the crates that `bindgen` and `cbindgen` need (85 MB), and caches the six small
Rust crates Mesa's NVK driver would otherwise download. The chroot has no network.

**A. The kernel (do this where the kernel is built, not on the running system).**
`kernel-profile.patch` adds `KERNEL_PROFILE` to `ch10-kernel-inside.sh`: `generic` (the new default) skips the
`localmodconfig` trimming and keeps all ~6,700 modules, signed; `laptop` is exactly the old behaviour. It also adds
`KERNEL_JOBS` (this laptop reached 97 °C and reset once during a build; use 4). The step wipes `/usr/lib/modules/<version>`
before installing, so it isn't one to run on the machine that is using those modules. Apply with
`patch /sources/ch10-kernel-inside.sh < kernel-profile.patch`.

**B. Everything else** is `step-hardware-generic-inside.sh`, in the style of the other step scripts (markers
`/sources/.done/9-<name>`, `ONLY="..."` to pick steps). It needs `step6-sourceable.patch` applied to
`step6-boot-inside.sh` first, so it can reuse that step's functions to rebuild the signed boot image.

| Step | What it does | Cost |
| --- | --- | --- |
| `firmware` | All of linux-firmware, zstd-compressed, minus server cards, datacenter switches and ARM chips (`FIRMWARE_SET=full` keeps them). Links into skipped folders are removed. | 492 MB (950 MB full), 30 s. Tested into a scratch folder. |
| `sof` | Intel sound DSP firmware for every platform, Intel-signed builds only. | 881 files. Tested. |
| `microcode` | Every Intel microcode file and AMD's, in one early image in `/boot/microcode.img`. | 18 MB; the boot image grows by that. Tested. |
| `libusb`, `pciutils`, `usbutils` | `lspci`, `setpci`, `lsusb`. pciutils is installed by hand so it doesn't overwrite hwdata's newer `pci.ids`. | Minutes. Built and run in a scratch prefix. |
| `rust-tools` | `bindgen` 0.73.2 and `cbindgen` 0.29.4, offline from the vendored crates. | 3 min. Built offline in scratch. |
| `llvm-amd` (`WITH_AMD=1` only) | Phase 8a's LLVM recipe, unchanged except `LLVM_TARGETS_TO_BUILD="host;AMDGPU"`. Mesa needs it for **any** AMD GPU support, however configured. | A full LLVM rebuild: hours. Off by default. |
| `mesa` | Phase 8a's Mesa plus `nouveau` (NVK, Vulkan) and `zink` (OpenGL on top of it): `gallium=iris,llvmpipe,zink`, `vulkan=intel,nouveau,swrast` (+ `amd`, `radeonsi` with `WITH_AMD=1`). | A full Mesa build. Configure checked on a fresh tree with only the cached crates. |
| `boot-image` | Rebuilds the initramfs and signed boot image with the new microcode. | Minutes. Not tested; it's the boot step's own commands. |

Run the steps that replace system libraries (`mesa`) and the boot image from the maintenance flow you already use,
and reboot after, as a running session loads Mesa.

## 4. NVIDIA on this laptop, after the above

With the generic kernel, udev loads `nouveau` for `10de:25a2` from its modalias with no help. nouveau uses the GSP
firmware on Ampere and newer, which the firmware step installs. Mesa's NVK gives Vulkan, and zink gives OpenGL. Games and
heavy programs on the NVIDIA GPU use `DRI_PRIME=1`; the desktop stays on the Intel GPU. **Not verified**: none of this has
run, since it needs the kernel and Mesa builds. The checks to do first are `autark-hw doctor` (the GPU should say Working), 
`vulkaninfo --summary` (with NVK listed), and watching the laptop's temperature and battery with the NVIDIA GPU awake,
as this is an MSI laptop that may not power the GPU down well.

## 5. The keyboard lights and the rest of the MSI controls

Two separate things, neither done, both blocked on facts that can't be guessed:

- **The embedded controller** (`msi_ec`: fan modes, shift mode, keyboard backlight, charge limit): **upstream's newer driver has this
  laptop's firmware** (checked 2026-10-19). BeardOverflow/msi-ec lists `15K1IMS1.113` explicitly, under the Cyborg 15 (A13VF), with its own
  register map (charge limit `0xd7`, fan modes `0xd4`, shift mode `0xd2`, keyboard backlight `0x2c` and `0xd3`). That is an entry for this firmware
  string, not another model's map borrowed in; this laptop is the A13UCK, so the one thing not known is whether the A13UCK's controller matches the
  A13VF's, which only loading it and reading the values can show. The kernel's own copy (7.1.8) has no `15K1` entry.
  `autark/scripts/msi-ec-module.sh` builds upstream at a pinned commit (the three files are checked against hashes), compiles it against the
  running kernel (works as any user, 2.5 s), signs it with the kernel's module key and puts it in `/usr/lib/modules/<kver>/updates/`, where
  modprobe prefers it to the kernel's own. `build` and `check` were run here; `install` and `load` need root and haven't been. Loading only reads
  the controller (one bit check at init); nothing is written until someone sets a fan mode, charge limit or backlight, and `remove` puts the
  kernel's own driver back. A kernel build wipes the module with the rest of `/usr/lib/modules/<kver>`, so run it again after one.
- **The RGB controller** (`1462:1601`, `MysticLight MS-1565`) has no published protocol that I could find; one OpenRGB
  report describes an MSI Mystic Light device becoming unresponsive after an effect was sent. Nothing is written to it.
  The Hardware window says plainly that it's a separate controller needing a driver profile.

When a verified definition exists, the intended shape is data, not code: a profile file matching the laptop by DMI or
USB ID that says which `hidraw` reports to send, plus a generated udev rule giving the logged-in user access to just
that device. That's how a new laptop would be supported without touching the program.

## Disk

`/home` was at 100% and 95% when this was written. About 2.3 GB is `~/autark-tools/target/debug`, which cargo rebuilds
and nothing here uses; `~/ollama-dl` is another 1.4 GB. Builds, downloads and firmware tests were done in `/tmp`.

## Files

| File | What |
| --- | --- |
| `kernel-profile.patch` | Adds `KERNEL_PROFILE=generic\|laptop` and `KERNEL_JOBS` to `ch10-kernel-inside.sh`. Applies cleanly; the result parses. |
| `step6-sourceable.patch` | One guard in `step6-boot-inside.sh` so `step-hardware-generic-inside.sh` can reuse its functions. |
| `step-hardware-generic-inside.sh` | The steps in section 3B. |
| `prepare-offline.sh` | Fetches and checks what the chroot can't download. Run end to end. |
| `install-helper.sh` | Installs the helper and `autark-hw` (root). |
