#!/usr/bin/env bash
# Script to fix all remaining Result<String, String> returns to Result<CallToolResult, McpError>

FILE="src/tools/search_tool.rs"

echo "Fixing return types in $FILE..."

# For the search method - fix the final returns (lines 448-454)
sed -i '448,460s/return Ok(format!/return Ok(CallToolResult::success(vec![Content::text(format!/g' "$FILE"
sed -i '448,460s/))$/)]))/g' "$FILE"
sed -i '468,470s/return Err(/return Err(McpError::internal(/g' "$FILE"
sed -i '193,205s/Ok(format!/Ok(CallToolResult::success(vec![Content::text(format!/g' "$FILE"
sed -i '193,205s/))$/)]))/g' "$FILE"

# Change all remaining function signatures from Result<String, String> to Result<CallToolResult, McpError>
sed -i 's/-> Result<String, String> {$/-> Result<CallToolResult, McpError> {/g' "$FILE"

# Fix simple Ok(format!(...)) patterns
sed -i 's/\bOk(format!/Ok(CallToolResult::success(vec![Content::text(format!/g' "$FILE"

# Fix the closing for those patterns - this is tricky, need to handle multi-line
# For now, let's handle single-line cases
sed -i 's/Content::text(format!(.*))$/Content::text(format!(\1)])) /g' "$FILE"

# Fix Err(format!(...)) patterns
sed -i 's/\bErr(format!/Err(McpError::internal(format!/g' "$FILE"

echo "Done! Now you need to manually fix multi-line cases and verify the file compiles."
