# Testing Guide - Rust Code MCP

This guide walks through testing all 8 MCP tools with MCP Inspector and Claude Desktop.

## Prerequisites

- ✅ Release binary built: `./target/release/file-search-mcp`
- Node.js installed (for MCP Inspector)
- Claude Desktop (optional, for real-world testing)

## Quick Test with MCP Inspector

### 1. Install MCP Inspector

```bash
npx @modelcontextprotocol/inspector ./target/release/file-search-mcp
```

This opens a web interface at `http://localhost:5173`

### 2. Test Each Tool

#### Tool 1: search
```json
{
  "directory": "/home/molaco/Documents/rust-code-mcp/src",
  "keyword": "parser"
}
```
**Expected:** List of files containing "parser"

#### Tool 2: read_file_content
```json
{
  "file_path": "/home/molaco/Documents/rust-code-mcp/src/lib.rs"
}
```
**Expected:** Contents of lib.rs

#### Tool 3: find_definition
```json
{
  "symbol_name": "RustParser",
  "directory": "/home/molaco/Documents/rust-code-mcp/src"
}
```
**Expected:** `src/parser/mod.rs:113 (struct)`

#### Tool 4: find_references
```json
{
  "symbol_name": "parse_file",
  "directory": "/home/molaco/Documents/rust-code-mcp/src"
}
```
**Expected:** Files and functions that call `parse_file`

#### Tool 5: get_dependencies
```json
{
  "file_path": "/home/molaco/Documents/rust-code-mcp/src/search/mod.rs"
}
```
**Expected:** List of imports from search/mod.rs

#### Tool 6: get_call_graph
```json
{
  "file_path": "/home/molaco/Documents/rust-code-mcp/src/parser/mod.rs"
}
```
**Expected:** Function call relationships in parser

**With specific symbol:**
```json
{
  "file_path": "/home/molaco/Documents/rust-code-mcp/src/parser/mod.rs",
  "symbol_name": "parse_source"
}
```

#### Tool 7: analyze_complexity
```json
{
  "file_path": "/home/molaco/Documents/rust-code-mcp/src/search/mod.rs"
}
```
**Expected:** Code metrics (LOC, complexity, symbol counts)

#### Tool 8: get_similar_code
**Note:** Requires Qdrant running and indexed chunks

```json
{
  "query": "parse rust code using tree-sitter",
  "directory": "/home/molaco/Documents/rust-code-mcp/src",
  "limit": 5
}
```
**Expected:** Similar code snippets with scores

## Testing with Claude Desktop

### 1. Configure Claude Desktop

Edit `~/.config/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "rust-code": {
      "command": "/home/molaco/Documents/rust-code-mcp/target/release/file-search-mcp"
    }
  }
}
```

### 2. Restart Claude Desktop

```bash
# Kill Claude if running
pkill -f "Claude"

# Start Claude Desktop
# (from your applications menu)
```

### 3. Test with Natural Language

Try these prompts in Claude:

**Finding code:**
```
Find where HybridSearch is defined in /home/molaco/Documents/rust-code-mcp
```

**Analyzing code:**
```
Analyze the complexity of src/search/mod.rs in my rust-code-mcp project
```

**Understanding dependencies:**
```
Show me all the imports in src/tools/search_tool.rs
```

**Call graph analysis:**
```
Show me the call graph for the parse_source function in src/parser/mod.rs
```

**Semantic search:**
```
Find code similar to "implementing MCP tools" in my project
```

## Expected Results Summary

### Working Tools (Should work immediately)

✅ **search** - File content indexing and search
✅ **read_file_content** - Read any file
✅ **find_definition** - Locate symbols via tree-sitter
✅ **find_references** - Find callers via call graph
✅ **get_dependencies** - List imports
✅ **get_call_graph** - Show call relationships
✅ **analyze_complexity** - Calculate metrics

### Tools Requiring Setup

⚠️ **get_similar_code** - Requires:
1. Qdrant server running
2. Chunks indexed in vector store

To set up:
```bash
# Start Qdrant (if not running)
docker run -p 6333:6333 qdrant/qdrant

# Or use embedded mode (no setup needed)
export QDRANT_MODE=embedded
```

## Debugging

### Enable Debug Logging

```bash
RUST_LOG=debug ./target/release/file-search-mcp
```

### Check MCP Communication

The MCP Inspector shows:
- Tool definitions
- Request/response JSON
- Errors and stack traces

### Common Issues

**Issue: "Tool not found"**
- Check tool name matches exactly
- Restart MCP client

**Issue: "Directory not found"**
- Use absolute paths
- Check directory exists

**Issue: "Qdrant connection failed" (get_similar_code)**
- Start Qdrant server
- Or use embedded mode: `export QDRANT_MODE=embedded`

**Issue: "No symbols found"**
- Make sure you're searching .rs files
- Check file has valid Rust syntax

## Validation Checklist

Test each tool and mark completion:

- [ ] search - Finds files by keyword
- [ ] read_file_content - Returns file contents
- [ ] find_definition - Locates symbol definitions
- [ ] find_references - Finds function callers
- [ ] get_dependencies - Lists imports
- [ ] get_call_graph - Shows call relationships
- [ ] analyze_complexity - Calculates metrics
- [ ] get_similar_code - Finds similar code (if Qdrant available)

## Performance Notes

Expected response times:
- **search**: <100ms (with persistent index)
- **read_file_content**: <10ms
- **find_definition**: 100-500ms (depends on codebase size)
- **find_references**: 100-500ms (depends on codebase size)
- **get_dependencies**: <50ms
- **get_call_graph**: <50ms
- **analyze_complexity**: <100ms
- **get_similar_code**: 200-1000ms (network + embedding)

## Next Steps After Testing

1. Document any bugs or issues
2. Note performance bottlenecks
3. Test with larger codebases
4. Try integration with other MCP clients
5. Proceed to Phase 8 (Optimization)

## Support

If you encounter issues:
1. Check RUST_LOG=debug output
2. Verify file paths are absolute
3. Ensure tools exist in MCP Inspector
4. Check Claude Desktop logs: `~/.config/Claude/logs/`
