#\!/bin/bash
# Test index_codebase tool

echo "Testing index_codebase on rust-code-mcp src directory..."
echo ""

echo "{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"tools/call\", \"params\": {\"name\": \"index_codebase\", \"arguments\": {\"directory\": \"/home/molaco/Documents/rust-code-mcp/src\"}}}" | \
  ./target/release/file-search-mcp 2>/dev/null | jq -r ".result.content[0].text"

echo ""
echo "✅ Indexing test complete\!"
