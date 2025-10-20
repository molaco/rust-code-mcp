#!/usr/bin/env bash
# Test script to verify MCP tools are properly advertised

set -e

echo "Testing MCP tools/list endpoint..."
echo ""

# Send JSON-RPC messages to the MCP server
(
  # 1. Initialize the connection
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}'

  # 2. Send initialized notification
  echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'

  # 3. Request tools list
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'

  # Give it time to process
  sleep 1
) | ./target/release/file-search-mcp 2>/dev/null | jq -c 'select(.id == 2)'

echo ""
echo "If you see tools listed above, the MCP server is working correctly!"
