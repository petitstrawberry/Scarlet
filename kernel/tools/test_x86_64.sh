#!/bin/bash

# Test script for x86_64 - wraps run_x86_64.sh with test-specific settings

export SCARLET_DEBUG_MODE=${SCARLET_TEST_DEBUG:-false}

# Pass all arguments through to the run script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$SCRIPT_DIR/run_x86_64.sh" "$@"
