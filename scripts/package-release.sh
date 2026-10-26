#!/bin/sh
set -eu

fail() {
    printf 'package-release: %s\n' "$1" >&2
    exit 1
}

[ "$#" -eq 3 ] || fail 'usage: package-release.sh TARGET VERSION OUTPUT_DIR'
target=$1
version=$2
output=$3
case "$target" in
    aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) fail 'unsupported release target' ;;
esac
printf '%s\n' "$version" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail 'version must be X.Y.Z'

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${CARGO_TARGET_DIR:-"$root/target"}
case "$target_dir" in
    /*) ;;
    *) target_dir="$PWD/$target_dir" ;;
esac
binary="$target_dir/$target/release/codex-commit"
[ -x "$binary" ] || fail "missing executable: $binary"
[ "$("$binary" --version)" = "codex-commit $version" ] || fail 'binary version does not match release version'

mkdir -p "$output"
output=$(CDPATH='' cd -- "$output" && pwd)
staging=$(mktemp -d "${TMPDIR:-/tmp}/codex-commit-package.XXXXXX")
trap 'rm -rf "$staging"' EXIT HUP INT TERM
install -m 755 "$binary" "$staging/codex-commit"
cp "$root/LICENSE" "$root/README.md" "$staging/"
archive="$output/codex-commit-v$version-$target.tar.gz"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$archive" -C "$staging" codex-commit LICENSE README.md
printf '%s\n' "$archive"
