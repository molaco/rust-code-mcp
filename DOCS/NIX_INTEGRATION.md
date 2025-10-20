# Nix Integration Guide

This document explains how to integrate rust-code-mcp with your existing Nix development environment.

## Your Current Setup

You already have a comprehensive Nix flake at `/home/molaco/Documents/nix-code/` with:
- Rust nightly toolchain
- Claude Code integration
- MCP servers via mcp-servers-nix
- Complete development environment

## Integration Options

### Option 1: Use Your Existing Nix Shell (Recommended)

Since you're already in a Nix dev shell with Rust nightly, you can develop rust-code-mcp directly:

```bash
# Already in your nix develop shell from /home/molaco/Documents/nix-code
cd /home/molaco/Documents/rust-code-mcp

# Build and run
cargo build --release
cargo test
```

**No changes needed** - your existing shell has everything required:
- ✅ Rust toolchain (nightly)
- ✅ pkg-config
- ✅ openssl
- ✅ sqlite
- ✅ All build dependencies

### Option 2: Add rust-code-mcp to Your Existing MCP Config

Add the rust-code-mcp server to your existing `.mcp.json` config:

#### Edit `/home/molaco/Documents/nix-code/shell.nix`

Add to the MCP servers configuration:

```nix
let
  config = mcp-servers-nix.lib.mkConfig pkgs {
    programs = {
      fetch.enable = true;
      context7.enable = true;
      sequential-thinking.enable = true;
      # ... existing servers
    };
    settings.servers = {
      # Add rust-code-mcp server
      rust-code = {
        command = "/home/molaco/Documents/rust-code-mcp/target/release/file-search-mcp";
        args = [];
        env = {
          QDRANT_MODE = "embedded";
          INDEX_PATH = "/home/molaco/.rust-code-mcp/index";
        };
      };
    };
  };
in
# ... rest of shell.nix
```

Then rebuild:
```bash
cd /home/molaco/Documents/nix-code
nix develop  # Rebuilds with new MCP server
```

### Option 3: Build as a Nix Package (Advanced)

If you want to package rust-code-mcp properly, you can add it to your flake:

#### Edit `/home/molaco/Documents/nix-code/flake.nix`

Add a new output:

```nix
outputs = { self, nixpkgs, rust-overlay, ... }@inputs:
  let
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
      overlays = [ rust-overlay.overlays.default ];
      config.allowUnfree = true;
    };
  in {
    devShells.${system}.default = import ./shell.nix { inherit pkgs; };

    # Add rust-code-mcp package
    packages.${system}.rust-code-mcp = pkgs.rustPlatform.buildRustPackage {
      pname = "rust-code-mcp";
      version = "0.1.0";
      src = /home/molaco/Documents/rust-code-mcp;

      cargoLock = {
        lockFile = /home/molaco/Documents/rust-code-mcp/Cargo.lock;
      };

      nativeBuildInputs = with pkgs; [ pkg-config ];
      buildInputs = with pkgs; [ openssl ];
    };
  };
```

Then use it:
```bash
nix build .#rust-code-mcp
# Binary at ./result/bin/file-search-mcp
```

## Recommended Approach

**Use Option 1** for now:
1. You already have all dependencies in your Nix shell
2. No need for a separate flake in rust-code-mcp/
3. Simple workflow: `cd rust-code-mcp && cargo build`

**Later (Phase 8+)**, when ready for production:
- Add rust-code-mcp as a package (Option 3)
- Integrate into your MCP config (Option 2)

## Dependencies Already Available

Your current shell provides:
```nix
buildInputs = with pkgs; [
  rustToolchain       # ✅ Rust nightly with rust-analyzer
  pkg-config          # ✅ Required for linking
  openssl             # ✅ Required for rmcp
  sqlite              # ✅ Can be used instead of sled
  cmake               # ✅ Might be needed for some dependencies
  # ... all other tools
];
```

## Phase 1 Dependencies

When you start Phase 1, these crates will work out of the box:
```toml
notify = "6"      # ✅ File watching - pure Rust
sled = "0.34"     # ✅ Metadata cache - pure Rust
sha2 = "0.10"     # ✅ File hashing - pure Rust
```

All are pure Rust and require no additional system dependencies.

## Phase 5 Dependencies (Qdrant)

For embedded Qdrant:
```toml
qdrant = "0.4"  # ✅ Pure Rust, no system deps needed
```

If you want remote Qdrant later:
```bash
# Option 1: From your nix shell
nix run nixpkgs#qdrant

# Option 2: Add to your shell.nix buildInputs
buildInputs = with pkgs; [
  # ... existing packages
  qdrant  # Qdrant server binary
];
```

## Testing Integration

To verify everything works:

```bash
# From your nix-code shell
cd /home/molaco/Documents/rust-code-mcp

# Should work without any additional setup
cargo build --release
cargo test
cargo run
```

## Environment Variables

Your shell already sets up paths correctly:
```nix
shellHook = ''
  export PATH=$PATH:${rustToolchain}/bin:$HOME/.cargo/bin
  # ... other exports
'';
```

For rust-code-mcp specific config, you can add to your shell:
```nix
shellHook = ''
  # ... existing hooks

  # Rust Code MCP config
  export RUST_CODE_MCP_INDEX_PATH="$HOME/.rust-code-mcp/index"
  export QDRANT_MODE="embedded"
'';
```

---

## Summary

**No separate flake needed** - your existing Nix setup is perfect for rust-code-mcp development!

Just use your current shell and build with cargo. Integration with your MCP config can come later when the server is production-ready.
