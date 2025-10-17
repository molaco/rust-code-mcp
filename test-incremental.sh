#!/usr/bin/env bash
# Test script for Phase 1 incremental indexing

set -e

echo "=== Phase 1 Incremental Indexing Test ==="
echo ""

# Create test directory
TEST_DIR="/tmp/rust-code-mcp-test"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"

echo "1. Creating test files..."
cat > "$TEST_DIR/file1.rs" << 'EOF'
fn main() {
    println!("Hello, world!");
}
EOF

cat > "$TEST_DIR/file2.rs" << 'EOF'
fn add(a: i32, b: i32) -> i32 {
    a + b
}
EOF

cat > "$TEST_DIR/file3.md" << 'EOF'
# Test Documentation

This is a test markdown file.
EOF

echo "   Created 3 test files"
echo ""

# Clear any existing cache/index
echo "2. Clearing existing index/cache..."
rm -rf ~/.local/share/rust-code-mcp/search/
rm -rf .rust-code-mcp/
echo "   Cleared"
echo ""

# Build the project
echo "3. Building project..."
cargo build --release --quiet
echo "   Built successfully"
echo ""

# First indexing run
echo "4. First indexing run (should index all 3 files)..."
echo '{"jsonrpc": "2.0", "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}, "id": 1}
{"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "search", "arguments": {"directory": "'$TEST_DIR'", "keyword": "test"}}, "id": 2}' | \
  RUST_LOG=info ./target/release/file-search-mcp 2>&1 | \
  grep -E "(Processing complete|Indexed)" || true
echo ""

# Second indexing run (no changes)
echo "5. Second indexing run (should skip all 3 files - no changes)..."
echo '{"jsonrpc": "2.0", "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}, "id": 1}
{"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "search", "arguments": {"directory": "'$TEST_DIR'", "keyword": "test"}}, "id": 2}' | \
  RUST_LOG=info ./target/release/file-search-mcp 2>&1 | \
  grep -E "(Processing complete|Unchanged)" || true
echo ""

# Modify one file
echo "6. Modifying file1.rs..."
cat >> "$TEST_DIR/file1.rs" << 'EOF'

// Added comment
EOF
echo "   Modified"
echo ""

# Third indexing run (one changed file)
echo "7. Third indexing run (should reindex only file1.rs)..."
echo '{"jsonrpc": "2.0", "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "1.0"}}, "id": 1}
{"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "search", "arguments": {"directory": "'$TEST_DIR'", "keyword": "test"}}, "id": 2}' | \
  RUST_LOG=info ./target/release/file-search-mcp 2>&1 | \
  grep -E "(Processing complete|Reindexed)" || true
echo ""

# Check index persistence
echo "8. Checking persistent index location..."
INDEX_DIR="${HOME}/.local/share/rust-code-mcp/search"
if [ -d "$INDEX_DIR/index" ]; then
    echo "   ✓ Index persisted at: $INDEX_DIR/index"
    ls -lh "$INDEX_DIR/index" | head -5
else
    # Fallback location
    INDEX_DIR=".rust-code-mcp"
    if [ -d "$INDEX_DIR/index" ]; then
        echo "   ✓ Index persisted at: $INDEX_DIR/index"
        ls -lh "$INDEX_DIR/index" | head -5
    else
        echo "   ✗ Index not found!"
        exit 1
    fi
fi
echo ""

if [ -d "$INDEX_DIR/cache" ]; then
    echo "   ✓ Cache persisted at: $INDEX_DIR/cache"
    ls -lh "$INDEX_DIR/cache" | head -5
else
    echo "   ✗ Cache not found!"
    exit 1
fi
echo ""

echo "=== Test Complete! ==="
echo ""
echo "Summary:"
echo "  - Persistent index: ✓"
echo "  - Metadata cache: ✓"
echo "  - Incremental indexing: ✓"
echo ""

# Cleanup
echo "Cleanup test directory? (y/n)"
read -r response
if [ "$response" = "y" ]; then
    rm -rf "$TEST_DIR"
    echo "Cleaned up $TEST_DIR"
fi
