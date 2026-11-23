#!/bin/sh
set -eu

previous=
for arg do
    if [ "$previous" = --target ] && [ "$arg" = aarch64-unknown-scarlet ]; then
        exec "$@" --sysroot "$SCARLET_IOS_GUEST_SYSROOT"
    fi
    previous=$arg
done
exec "$@"
