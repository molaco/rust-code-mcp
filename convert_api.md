# API Conversion Guide

## What Needs to Change

The file `/home/molaco/Documents/rust-code-mcp/src/tools/search_tool.rs` needs these updates:

### Already Done ✓
1. Imports updated (lines 1-9)
2. Struct has tool_router field (lines 96-106)
3. Helper methods separated into own impl block (lines 101-146)
4. Tool router impl block started (line 148)

### Still Needed

Each of the 8 tool functions needs:

1. **read_file_content** (line 152): 
   - Signature: Already updated ✓
   - Body: Change `params.file_path` → `file_path` (lines 157-213)
   - Returns: Change `Ok(content)` → `Ok(CallToolResult::success(vec![Content::text(content)]))`
   - Errors: Change `Err(format!(...))` → `Err(McpError::invalid_params(..., None))`

2. **search** (line 220):
   - Signature: `#[tool(aggr)] params: SearchParams` → `Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>`
   - Body: `params.directory` → `directory`, `params.keyword` → `keyword`
   - Returns: Same pattern

3-8. **find_definition, find_references, get_dependencies, get_call_graph, analyze_complexity, get_similar_code**:
   - Same pattern for each

### ServerHandler (line 777-793)
Replace:
```rust
#[tool(tool_box)]
impl ServerHandler for SearchTool {
```

With:
```rust
#[tool_handler]
impl ServerHandler for SearchTool {
```

## Quick Fix Script

Due to the size and complexity, here are the exact manual steps or use the provided conversion script.
