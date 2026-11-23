#!/bin/sh
set -eu

previous=
for arg do
    case "$previous:$arg" in
        --target:aarch64-unknown-scarlet|*:--target=aarch64-unknown-scarlet)
            # Some source-owned app recipes supply their own RUSTFLAGS. Keep
            # the platform ABI and segment layout consistent for those too.
            exec "$@" --sysroot "$SCARLET_IOS_GUEST_SYSROOT" -Zfixed-x18 \
                -C relocation-model=pic \
                -C link-arg=-z -C link-arg=max-page-size=16384 \
                -C link-arg=-z -C link-arg=separate-loadable-segments
            ;;
    esac
    previous=$arg
done
exec "$@"
