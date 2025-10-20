#!/usr/bin/env bash
# Test to see exactly what capabilities are returned

echo "Testing MCP capabilities response..."
echo ""

# Send initialize and check what capabilities are returned
(
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}'
  sleep 0.5
) | ./target/release/file-search-mcp 2>/dev/null | jq '.result.capabilities'
