#!/bin/sh
# Installs the hardware helper and the Hardware app: the helper and its D-Bus, systemd and polkit files, and the
# autark-hw tool that talks to it. Run it as root once. Until it has been, the Hardware window works but can't load a
# driver or change a power profile or charge limit that this user can't change on their own.
#
#   su -c 'sh hardware/install-helper.sh'   (from the repository's root)
#
# BIN is where the built binaries are (default: the project's target/release). Nothing here is removed or changed except
# the files it names; uninstall.sh-style removal is `rm` of exactly the paths printed.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${BIN:-$HERE/../target/release}
DATA=$HERE/../crates/autark-hw-helper/data

for f in "$BIN/autark-hw-helper" "$BIN/autark-hw"; do
  [ -x "$f" ] || { echo "install-helper.sh: $f isn't built (BIN=$BIN)" >&2; exit 1; }
done
[ "$(id -u)" = 0 ] || { echo "install-helper.sh: run as root" >&2; exit 1; }

install -vDm755 "$BIN/autark-hw-helper"            /usr/libexec/autark-hw-helper
install -vDm755 "$BIN/autark-hw"                   /usr/bin/autark-hw
install -vDm644 "$DATA/autark-hw-helper.service"   /usr/lib/systemd/system/autark-hw-helper.service
install -vDm644 "$DATA/org.autark.Hardware1.service" /usr/share/dbus-1/system-services/org.autark.Hardware1.service
install -vDm644 "$DATA/org.autark.Hardware1.conf"  /usr/share/dbus-1/system.d/org.autark.Hardware1.conf
install -vDm644 "$DATA/org.autark.hardware.policy" /usr/share/polkit-1/actions/org.autark.hardware.policy
install -vDm644 "$HERE/../crates/autark-hw/data/autark-hw.desktop" /usr/share/applications/autark-hw.desktop

systemctl daemon-reload
# D-Bus picks up the new policy and activation file without a restart.
systemctl reload dbus.service 2>/dev/null || busctl --system call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig 2>/dev/null || true
echo "== hardware helper installed. Check it with: busctl --system call org.autark.Hardware1 /org/autark/Hardware1 org.autark.Hardware1 Loadable"
