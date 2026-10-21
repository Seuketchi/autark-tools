#!/bin/sh
# Installs the Hub's root helper and makes sure sudo works for you. Run it once, as root:
#   su -c 'sh ~/autark-tools/crates/autark-hub/data/install-helper.sh'
# It puts autark-hub-helper in /usr/libexec/autark/ and adds a rule letting the wheel group use sudo if nothing grants it
# (Settings' helper does the same; whichever you run first sets it up). The helper can only run `autark-pkg` with the verbs
# install, remove, upgrade and rollback, on package names, and never imports from Arch Linux.
set -eu

[ "$(id -u)" = 0 ] || { echo "install-helper.sh: run it as root (su -c 'sh $0')" >&2; exit 1; }

here=$(cd "$(dirname "$0")" && pwd)
bin="$here/../../../target/release/autark-hub-helper"
[ -x "$bin" ] || { echo "install-helper.sh: build it first: cargo build --release -p autark-hub ($bin)" >&2; exit 1; }

install -d -m 755 -o root -g root /usr/libexec/autark
install -m 755 -o root -g root "$bin" /usr/libexec/autark/autark-hub-helper
echo "installed /usr/libexec/autark/autark-hub-helper"

if ! grep -Eqs '^[[:space:]]*%wheel[[:space:]]' /etc/sudoers /etc/sudoers.d/*; then
  printf '%%wheel ALL=(ALL:ALL) ALL\n' > /etc/sudoers.d/10-wheel
  chmod 440 /etc/sudoers.d/10-wheel
  echo "added /etc/sudoers.d/10-wheel: members of wheel may use sudo with their password"
fi

visudo -c
echo "done: the Hub can now install, remove, update and roll back packages"
