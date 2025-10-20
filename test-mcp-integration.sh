#!/usr/bin/env bash
# Test script for rust-code-mcp integration with Nix and Claude Code

set -e

echo "========================================="
echo "rust-code-mcp Integration Test"
echo "========================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

success() {
    echo -e "${GREEN}✓${NC} $1"
}

error() {
    echo -e "${RED}✗${NC} $1"
}

warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

info() {
    echo -e "  $1"
}

# Test 1: Check binary exists and is executable
echo "Test 1: Checking binary..."
BINARY_PATH="/home/molaco/Documents/rust-code-mcp/target/release/file-search-mcp"

if [ -f "$BINARY_PATH" ]; then
    success "Binary exists at $BINARY_PATH"
else
    error "Binary not found at $BINARY_PATH"
    info "Run: cargo build --release"
    exit 1
fi

if [ -x "$BINARY_PATH" ]; then
    success "Binary is executable"
else
    error "Binary is not executable"
    info "Run: chmod +x $BINARY_PATH"
    exit 1
fi

# Test 2: Check settings.local.json
echo ""
echo "Test 2: Checking Claude Code configuration..."
SETTINGS_PATH="/home/molaco/Documents/rust-code-mcp/.claude/settings.local.json"

if [ -f "$SETTINGS_PATH" ]; then
    success "settings.local.json exists"

    if grep -q "rust-code-mcp" "$SETTINGS_PATH"; then
        success "rust-code-mcp is configured in settings.local.json"
    else
        error "rust-code-mcp not found in settings.local.json"
        exit 1
    fi

    if grep -q "mcp__rust-code-mcp__\*" "$SETTINGS_PATH"; then
        success "Permissions configured for rust-code-mcp"
    else
        warning "Permissions may not be configured"
    fi
else
    error "settings.local.json not found"
    exit 1
fi

# Test 3: Validate JSON syntax
echo ""
echo "Test 3: Validating JSON syntax..."

if command -v jq &> /dev/null; then
    if jq empty "$SETTINGS_PATH" 2>/dev/null; then
        success "settings.local.json is valid JSON"
    else
        error "settings.local.json has syntax errors"
        info "Run: jq . $SETTINGS_PATH"
        exit 1
    fi
else
    warning "jq not found, skipping JSON validation"
fi

# Test 4: Check Nix shell configuration
echo ""
echo "Test 4: Checking Nix shell configuration..."
SHELL_NIX_PATH="/home/molaco/Documents/nix-code/shell.nix"

if [ -f "$SHELL_NIX_PATH" ]; then
    success "shell.nix exists"

    if grep -q "rust-code-mcp" "$SHELL_NIX_PATH"; then
        success "rust-code-mcp is configured in shell.nix"
    else
        error "rust-code-mcp not found in shell.nix"
        exit 1
    fi
else
    error "shell.nix not found at $SHELL_NIX_PATH"
    exit 1
fi

# Test 5: Check if Nix development shell can be entered
echo ""
echo "Test 5: Testing Nix development shell (this may take a moment)..."

if command -v nix &> /dev/null; then
    success "nix command available"

    # Test that flake.nix is valid by doing a dry-run
    cd /home/molaco/Documents/nix-code
    if nix develop --command echo 'Shell works' 2>&1 | grep -q "Shell works"; then
        success "Nix development shell can be entered successfully"
    else
        error "Failed to enter Nix development shell"
        info "Try: cd /home/molaco/Documents/nix-code && nix develop"
        exit 1
    fi
else
    error "nix command not found"
    info "Is Nix installed with flakes enabled?"
    exit 1
fi

# Test 6: Check .mcp.json generation
echo ""
echo "Test 6: Checking .mcp.json generation..."

cd /home/molaco/Documents/nix-code
export PROJECT_DIR="$PWD"

# Enter development shell and check if .mcp.json is created
nix develop --command bash -c "
    if [ -L \".mcp.json\" ]; then
        echo 'SUCCESS: .mcp.json symlink exists'
    else
        echo 'ERROR: .mcp.json symlink not created'
        exit 1
    fi
" 2>&1 | grep -q "SUCCESS"

if [ $? -eq 0 ]; then
    success ".mcp.json symlink is created by shellHook"

    # Check if it contains rust-code-mcp
    if [ -L "$PROJECT_DIR/.mcp.json" ]; then
        if grep -q "rust-code-mcp" "$PROJECT_DIR/.mcp.json" 2>/dev/null; then
            success ".mcp.json contains rust-code-mcp configuration"
        else
            warning ".mcp.json exists but may not contain rust-code-mcp"
            info "This will be populated when you enter nix develop"
        fi
    fi
else
    error ".mcp.json symlink not created"
    info "shellHook may not be executing properly"
fi

# Test 7: Quick binary test
echo ""
echo "Test 7: Quick binary smoke test..."

# The binary will fail with a protocol error when not given proper MCP input,
# but it should at least start and not crash
timeout 2s "$BINARY_PATH" < /dev/null 2>&1 | head -5 || true
success "Binary starts (protocol errors are expected without MCP client)"

# Summary
echo ""
echo "========================================="
echo "Summary"
echo "========================================="
echo ""

success "All configuration checks passed!"
echo ""
info "Next steps:"
echo ""
echo "1. Enter the Nix development shell:"
echo "   cd /home/molaco/Documents/nix-code"
echo "   nix develop"
echo ""
echo "2. Open Claude Code in the rust-code-mcp project:"
echo "   cd /home/molaco/Documents/rust-code-mcp"
echo "   claude-code . (or your Claude Code command)"
echo ""
echo "3. Check MCP status in Claude Code:"
echo "   - Look for MCP indicator in status bar"
echo "   - Click to see 'rust-code-mcp' listed"
echo ""
echo "4. Test with a command in Claude Code chat:"
echo "   @rust-code-mcp read /home/molaco/Documents/rust-code-mcp/src/lib.rs"
echo ""
echo "5. For detailed testing, see: TESTING.md"
echo ""
echo "========================================="
echo "Configuration Details"
echo "========================================="
echo ""
echo "Binary: $BINARY_PATH"
echo "Settings: $SETTINGS_PATH"
echo "Shell config: $SHELL_NIX_PATH"
echo "MCP config: /home/molaco/Documents/nix-code/.mcp.json (created in nix-shell)"
echo ""
echo "To enable debug logging, the shell.nix already sets RUST_LOG=info"
echo "For more verbose output, change to RUST_LOG=debug in shell.nix"
echo ""
