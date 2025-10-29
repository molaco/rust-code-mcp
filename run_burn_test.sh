#!/usr/bin/env bash
# Run the Burn codebase GPU performance test
#
# This script sets the required environment variables to avoid stack overflow

echo "Running Burn GPU performance test..."
echo "This will take approximately 6-7 minutes for 1,616 files"
echo ""

RUST_MIN_STACK=8388608 cargo test --test test_burn_performance test_burn_gpu_performance -- --ignored --nocapture
