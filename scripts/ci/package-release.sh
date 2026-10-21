#!/bin/bash
# Packs the built programs and their desktop files and icons into one tarball with a checksum file.
#   scripts/ci/package-release.sh <tag> [debug|release] [output dir]
# Nothing is signed here: releases are signed by hand, offline (see docs/specs/releasing.md in the autark repository).
set -euo pipefail
tag=${1:?usage: package-release.sh <tag> [debug|release] [output dir]}
profile=${2:-release}
out=${3:-dist}
cd "$(git rev-parse --show-toplevel)"
name="autark-tools-$tag-x86_64-linux"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root="$stage/$name"
mkdir -p "$root/bin" "$root/share/applications" "$root/share/icons"

# Every program the workspace builds: the executables named autark-* in the profile's folder (not the .d files or
# libraries), except the two developer tools of autark-init.
count=0
while IFS= read -r f; do
  install -m 755 "$f" "$root/bin/"
  count=$((count + 1))
done < <(find "target/$profile" -maxdepth 1 -type f -perm -u+x -name 'autark-*' ! -name '*.d' ! -name 'autark-init-test-payload' ! -name 'autark-init-splash-preview' | sort)
[ "$count" -gt 0 ] || { echo "package-release: no programs in target/$profile; build first (cargo build --$profile --workspace --locked)" >&2; exit 1; }

find crates -path '*/data/*.desktop' -exec install -m 644 {} "$root/share/applications/" \;
find crates -path '*/data/*.svg' -exec install -m 644 {} "$root/share/icons/" \;
cp LICENSE-MIT LICENSE-APACHE "$root/"
cat > "$root/README.txt" <<TXT
autark-tools $tag, built on Arch Linux (x86-64, glibc as in Arch at the time of the build).

These are convenience builds for other wlroots desktops. They are NOT the packages Autark itself installs: Autark builds
its own from recipes with its own libraries and flags, and signs them with the release key. These tarballs are not signed.
Check the download against SHA256SUMS, and read the source before you run anything as root.

Programs are in bin/, desktop entries in share/applications/, icons in share/icons/.
The root helpers (autark-settings-helper, autark-hub-helper, autark-hw-helper) go in /usr/libexec/autark/; the
install-helper.sh scripts in the source tree show how.
TXT

mkdir -p "$out"
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner -cf - "$name" | zstd -19 -T0 -q -o "$out/$name.tar.zst"
(cd "$out" && sha256sum "$name.tar.zst" > SHA256SUMS)
echo "packed $count programs: $out/$name.tar.zst"
cat "$out/SHA256SUMS"
