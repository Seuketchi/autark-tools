#!/bin/sh
# Installs Settings' root helper and makes sure sudo works for you. Run it once, as root:
#   su -c 'sh ~/autark-tools/crates/autark-settings/data/install-helper.sh'
# It does three things and says so: puts autark-settings-helper in /usr/libexec/autark/, removes
# /etc/sudoers.d/8c-build if sudo can't parse it (it was written with a stray EOF line and stops sudo for everyone), and
# adds a rule letting the wheel group use sudo if nothing grants it. Then it checks the sudoers files.
set -eu

[ "$(id -u)" = 0 ] || { echo "install-helper.sh: run it as root (su -c 'sh $0')" >&2; exit 1; }

here=$(cd "$(dirname "$0")" && pwd)
bin="$here/../../../target/release/autark-settings-helper"
[ -x "$bin" ] || { echo "install-helper.sh: build it first: cargo build --release -p autark-settings ($bin)" >&2; exit 1; }

install -d -m 755 -o root -g root /usr/libexec/autark
install -m 755 -o root -g root "$bin" /usr/libexec/autark/autark-settings-helper
echo "installed /usr/libexec/autark/autark-settings-helper"

if [ -f /etc/sudoers.d/8c-build ] && ! visudo -cf /etc/sudoers.d/8c-build >/dev/null 2>&1; then
  rm /etc/sudoers.d/8c-build
  echo "removed the broken /etc/sudoers.d/8c-build"
fi

if ! grep -Eqs '^[[:space:]]*%wheel[[:space:]]' /etc/sudoers /etc/sudoers.d/*; then
  printf '%%wheel ALL=(ALL:ALL) ALL\n' > /etc/sudoers.d/10-wheel
  chmod 440 /etc/sudoers.d/10-wheel
  echo "added /etc/sudoers.d/10-wheel: members of wheel may use sudo with their password"
fi

visudo -c
echo "done: Settings can now change DNS, Bluetooth, the firewall, accounts and updates"
