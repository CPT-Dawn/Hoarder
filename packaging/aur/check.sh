#!/usr/bin/env bash
# Build rendered AUR packages from local release artifacts and lint them:
#
#   packaging/aur/check.sh RENDERED ARTIFACTS PKGDEST PKGNAME...
#
# RENDERED is render.sh's OUTDIR. Sources are taken from ARTIFACTS (makepkg
# finds them in SRCDEST and downloads nothing), so this works before the
# release exists. Each package is built with `makepkg --cleanbuild` and then
# checked with namcap, PKGBUILD and package both; any namcap error fails.
# Run it as an unprivileged user; it never installs anything.
set -euo pipefail

if [ $# -lt 4 ]; then
	echo "usage: $0 RENDERED ARTIFACTS PKGDEST PKGNAME..." >&2
	exit 2
fi
rendered=$(cd "$1" && pwd) artifacts=$(cd "$2" && pwd)
mkdir -p "$3"
pkgdest=$(cd "$3" && pwd)
shift 3

lint() {
	local out
	out=$(namcap "$@")
	[ -z "$out" ] || printf '%s\n' "$out"
	# namcap prints "E:" for errors and "W:" for warnings.
	! grep -q ' E: ' <<<"$out"
}

for pkg in "$@"; do
	echo "==> $pkg"
	build=$(mktemp -d)
	cp -r "$rendered/$pkg/." "$build/"
	lint "$build/PKGBUILD"
	flags=(--cleanbuild --force --noconfirm)
	rules=()
	# The shims need a hoarder package only to run, not to build. namcap
	# checks their symlinks against the installed system, so without one
	# installed (CI installs hoarder-bin first) that rule is skipped.
	if [ "$pkg" = hoarder-wl-compat ]; then
		flags+=(--nodeps)
		if ! pacman -T hoarder >/dev/null; then
			echo "(no hoarder installed: not checking where the shims point)"
			rules=(-e symlink)
		fi
	fi
	(cd "$build" && SRCDEST="$artifacts" PKGDEST="$pkgdest" makepkg "${flags[@]}")
	for file in "$pkgdest/$pkg"-[0-9]*.pkg.tar*; do
		echo "--> $(basename "$file")"
		lint "${rules[@]}" "$file"
		bsdtar -tf "$file" | grep -v '^\.' | sort
	done
	rm -rf "$build"
done
