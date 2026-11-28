#!/bin/sh
set -eu

destination=$1
project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source="$project/bsp/tests"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
clang=$(/usr/bin/xcrun --sdk iphonesimulator --find clang)
set -- -z max-page-size=16384 -z separate-loadable-segments

case "${destination##*/}" in
    libboot-check.so|libboot-check-alt.so)
        alternate=
        if [ "${destination##*/}" = libboot-check-alt.so ]; then
            alternate=-DSCARLET_ALTERNATE_LIBRARY
        fi
        "$clang" $alternate -target aarch64-none-elf -c "$source/boot_library.S" -o "$work/object.o"
        ld.lld -shared "$@" -soname libboot-check.so "$work/object.o" -o "$destination"
        ;;
    boot-check)
        "$clang" -target aarch64-none-elf -c "$source/boot_fixture.S" -o "$work/object.o"
        ld.lld -pie "$@" --dynamic-linker=/bin/scarlet-ld -e _scarlet_boot_start \
            --export-dynamic --unresolved-symbols=ignore-all \
            "$work/object.o" "$(dirname "$destination")/../lib/libboot-check.so" -o "$destination"
        ;;
    *) echo "Unknown boot fixture: $destination" >&2; exit 2 ;;
esac
printf '\123' | dd of="$destination" bs=1 seek=7 conv=notrunc 2>/dev/null
