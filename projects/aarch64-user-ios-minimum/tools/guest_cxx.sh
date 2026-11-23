#!/bin/sh
set -eu
exec "$SCARLET_IOS_NATIVE_CXX" -ffixed-x18 "$@"
