#!/bin/bash
# Usage: extract-kernel-symbols.sh <kernel-elf> <output-rs>
set -euo pipefail

KERNEL_ELF="$1"
OUTPUT_RS="$2"

if [ ! -f "$KERNEL_ELF" ]; then
    echo "Error: kernel ELF not found: $KERNEL_ELF" >&2
    exit 1
fi

nm "$KERNEL_ELF" --defined-only --extern-only -g --no-sort 2>/dev/null | while read -r addr type name; do
    [ -z "$name" ] && continue
    case "$name" in
        _GLOBAL_OFFSET_TABLE_|_DYNAMIC|__.*_START|__.*_END) continue ;;
    esac
    printf '%s\t%s\n' "$name" "$addr"
done > /tmp/lsm_syms_$$.txt

{
    echo "static KERNEL_SYMBOLS: &[(&str, usize)] = &["
    while IFS=$'\t' read -r name addr; do
        printf '    ("%s", 0x%s),\n' "$name" "$addr"
    done < /tmp/lsm_syms_$$.txt
    echo "];"
    echo
    echo "pub fn get_kernel_symbols() -> &'static [(&'static str, usize)] { KERNEL_SYMBOLS }"
} > "$OUTPUT_RS"

rm -f /tmp/lsm_syms_$$.txt

echo "Generated $OUTPUT_RS with $(grep -c '0x' "$OUTPUT_RS" || true) symbols"
