#!/bin/bash
# Test type reference tracking for RustParser

echo "Testing find_references for 'RustParser' with type tracking..."
echo ""

echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "find_references", "arguments": {"symbol_name": "RustParser", "directory": "/home/molaco/Documents/rust-code-mcp/src"}}}' | \
  ./target/release/file-search-mcp 2>/dev/null | jq -r '.result.content[0].text'

echo ""
echo "✅ Test complete!"
