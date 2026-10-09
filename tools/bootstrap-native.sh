#!/bin/sh
# Rebuild the complete Ryn-written compiler using an existing native rync.
# Every compiler process stays under 1 GiB; stage2 and stage3 must be identical.
set -eu
if [ "$#" -ne 2 ]; then
    echo "usage: $0 /absolute/path/to/rync /absolute/output/directory" >&2
    exit 2
fi
seed=$1
output=$2
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "$output"
output=$(CDPATH= cd -- "$output" && pwd)
if [ -e "$output/rync-stage2" ] || [ -e "$output/rync-stage3" ]; then
    echo "bootstrap outputs already exist: $output" >&2
    exit 2
fi
cd "$root"
(ulimit -v 1048576; "$seed" --stdlib "$root/stdlib/std/src" "$root/selfhost/src/rync.ryn" > "$output/rync-stage2")
chmod +x "$output/rync-stage2"
(ulimit -v 1048576; "$output/rync-stage2" --stdlib "$root/stdlib/std/src" "$root/selfhost/src/rync.ryn" > "$output/rync-stage3")
chmod +x "$output/rync-stage3"
cmp "$output/rync-stage2" "$output/rync-stage3"
sha256sum "$output/rync-stage2" "$output/rync-stage3"
