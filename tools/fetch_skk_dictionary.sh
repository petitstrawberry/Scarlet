#!/usr/bin/env sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SIZE=${SKK_JISYO_SIZE:-L}
URL=${SKK_JISYO_URL:-https://skk-dev.github.io/dict/SKK-JISYO.${SIZE}.gz}
OUT=${1:-"$ROOT_DIR/.scarlet/cache/skk/SKK-JISYO.${SIZE}"}

tmp_gz=$(mktemp)
tmp_euc=$(mktemp)
trap 'rm -f "$tmp_gz" "$tmp_euc"' EXIT HUP INT TERM

download() {
    url=$1
    out=$2

    if command -v curl >/dev/null 2>&1; then
        curl -fL "$url" -o "$out"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$out" "$url"
    else
        echo "fetch_skk_dictionary: curl or wget is required" >&2
        exit 1
    fi
}

find_iconv() {
    for candidate in /usr/bin/iconv /bin/iconv iconv; do
        if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -l >/dev/null 2>&1; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

if ! command -v gzip >/dev/null 2>&1; then
    echo "fetch_skk_dictionary: gzip is required" >&2
    exit 1
fi

ICONV=$(find_iconv) || {
    echo "fetch_skk_dictionary: iconv is required for EUC-JP to UTF-8 conversion" >&2
    exit 1
}

echo "fetch_skk_dictionary: downloading $URL"
download "$URL" "$tmp_gz"
gzip -dc "$tmp_gz" > "$tmp_euc"

mkdir -p "$(dirname -- "$OUT")"
"$ICONV" -f EUC-JP -t UTF-8 "$tmp_euc" > "$OUT"

echo "fetch_skk_dictionary: wrote $OUT"
