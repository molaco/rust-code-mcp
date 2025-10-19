#!/usr/bin/env bash

# Test the search MCP tool
DIRECTORY="/home/molaco/Documents/rust-code-mcp"
KEYWORD="UnifiedIndexer"

echo "=== Testing MCP Search Tool ==="
echo "Directory: $DIRECTORY"
echo "Keyword: $KEYWORD"
echo ""
echo "Sending MCP request..."
echo ""

# Call via MCP protocol
echo '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "search",
    "arguments": {
      "directory": "'$DIRECTORY'",
      "keyword": "'$KEYWORD'"
    }
  }
}' | cargo run --release --bin file-search-mcp 2>&1

echo ""
echo "=== Test Complete ==="
