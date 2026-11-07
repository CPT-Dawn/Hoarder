#!/usr/bin/env bash
# Pack a release binary into the tarball the GitHub release and `hoarder-bin`
# ship:
#
#   packaging/dist.sh BINARY VERSION TARGET OUTDIR
#
# writes OUTDIR/hoarder-VERSION-TARGET.tar.gz and its .sha256. The tarball
# holds one directory, hoarder-VERSION-TARGET/, with the binary, LICENSE,
# README.md and what `hoarder dist` lays out (man pages, completions, the
# systemd user unit, the default config). BINARY must run on this machine.
set -euo pipefail

if [ $# -ne 4 ]; then
	echo "usage: $0 BINARY VERSION TARGET OUTDIR" >&2
	exit 2
fi
bin=$1 version=$2 target=$3 out=$4
repo=$(cd "$(dirname "$0")/.." && pwd)
name="hoarder-$version-$target"

# The binary has to be the version being released.
reported=$("$bin" --version)
if [ "$reported" != "hoarder $version" ]; then
	echo "$bin reports '$reported', not 'hoarder $version'" >&2
	exit 1
fi

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root="$stage/$name"
install -Dm755 "$bin" "$root/hoarder"
"$bin" dist "$root" --bindir /usr/bin
install -m644 "$repo/LICENSE" "$repo/README.md" "$root/"

mkdir -p "$out"
out=$(cd "$out" && pwd)
# Reproducible: fixed owner, order and mtime.
tar -C "$stage" --owner=0 --group=0 --numeric-owner --sort=name \
	--mtime="@${SOURCE_DATE_EPOCH:-0}" -cf - "$name" | gzip -9n >"$out/$name.tar.gz"
(cd "$out" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
echo "$out/$name.tar.gz"
