#!/bin/sh
set -eu
exec "$SCARLET_IOS_NATIVE_CC" -ffixed-x18 "$@"
