#!/usr/bin/env bash
# Turn packaging/aur/ into the four AUR repositories for a release:
#
#   packaging/aur/render.sh VERSION ARTIFACTS OUTDIR
#
# ARTIFACTS holds the release's hoarder-VERSION.tar.gz (source) and
# hoarder-VERSION-{x86_64,aarch64}-unknown-linux-gnu.tar.gz (binaries).
# OUTDIR/<pkgname>/ gets PKGBUILD (pkgver, pkgrel=1 and real checksums) and
# .SRCINFO, ready to commit to ssh://aur@aur.archlinux.org/<pkgname>.git.
# hoarder-git keeps its pkgver() and only gets a fresh .SRCINFO.
# Needs makepkg (Arch) for .SRCINFO.
set -euo pipefail

if [ $# -ne 3 ]; then
	echo "usage: $0 VERSION ARTIFACTS OUTDIR" >&2
	exit 2
fi
version=$1 artifacts=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)

sha() { sha256sum "$artifacts/$1" | cut -d' ' -f1; }
src=$(sha "hoarder-$version.tar.gz")
x86_64=$(sha "hoarder-$version-x86_64-unknown-linux-gnu.tar.gz")
aarch64=$(sha "hoarder-$version-aarch64-unknown-linux-gnu.tar.gz")

for pkg in hoarder hoarder-bin hoarder-git hoarder-wl-compat; do
	mkdir -p "$out/$pkg"
	cp "$here/$pkg/PKGBUILD" "$out/$pkg/PKGBUILD"
done
cp "$repo/LICENSE" "$out/hoarder-wl-compat/LICENSE"
license=$(sha256sum "$repo/LICENSE" | cut -d' ' -f1)

release() {
	sed -i -e "s/^pkgver=.*/pkgver=$version/" -e "s/^pkgrel=.*/pkgrel=1/" "$out/$1/PKGBUILD"
}
release hoarder
sed -i "s/^sha256sums=.*/sha256sums=('$src')/" "$out/hoarder/PKGBUILD"
release hoarder-bin
sed -i -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$x86_64')/" \
	-e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('$aarch64')/" "$out/hoarder-bin/PKGBUILD"
release hoarder-wl-compat
sed -i "s/^sha256sums=.*/sha256sums=('$license')/" "$out/hoarder-wl-compat/PKGBUILD"

for pkg in hoarder hoarder-bin hoarder-git hoarder-wl-compat; do
	(cd "$out/$pkg" && makepkg --printsrcinfo >.SRCINFO)
	if grep -q "SKIP" "$out/$pkg/PKGBUILD" && [ "$pkg" != hoarder-git ]; then
		echo "$pkg: a checksum is still SKIP" >&2
		exit 1
	fi
done
echo "Rendered $out/{hoarder,hoarder-bin,hoarder-git,hoarder-wl-compat}"
