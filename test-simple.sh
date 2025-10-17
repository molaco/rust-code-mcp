#!/usr/bin/env bash
# Simple test for Phase 1 features

set -e

echo "=== Simple Incremental Index Test ==="
echo ""

# Create test directory
TEST_DIR="/tmp/rust-mcp-simple-test"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"

echo "Creating test files..."
echo "fn test1() { println!(\"test\"); }" > "$TEST_DIR/test1.rs"
echo "fn test2() { println!(\"hello\"); }" > "$TEST_DIR/test2.rs"
echo "# Documentation" > "$TEST_DIR/README.md"
echo "   Created 3 files"
echo ""

# Clear cache
echo "Clearing cache..."
rm -rf ~/.local/share/rust-code-mcp/
rm -rf .rust-code-mcp/
echo ""

# Show what we're going to test
echo "Directory structure:"
ls -la "$TEST_DIR"
echo ""

echo "Index will be created at one of:"
echo "  - ~/.local/share/rust-code-mcp/search/"
echo "  - ./.rust-code-mcp/"
echo ""

echo "To test manually:"
echo "1. Add this to your Claude .mcp.json:"
echo '   "rust-code": {'
echo '     "command": "'$(pwd)'/target/release/file-search-mcp"'
echo '   }'
echo ""
echo "2. In Claude, run: search for 'test' in '$TEST_DIR'"
echo "3. Run again - should skip unchanged files"
echo "4. Modify $TEST_DIR/test1.rs"
echo "5. Search again - should only reindex test1.rs"
echo ""

echo "Or test with MCP inspector:"
echo "  npx @modelcontextprotocol/inspector ./target/release/file-search-mcp"
echo ""
