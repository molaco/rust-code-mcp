{
  description = "rust-code-mcp - Semantic code search MCP server for Rust codebases";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    mcp-servers-nix.url = "github:natsukium/mcp-servers-nix";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      mcp-servers-nix,
      ...
    }:
    let
      system = "x86_64-linux";
      overlays = [ rust-overlay.overlays.default ];

      pkgs = import nixpkgs {
        inherit system overlays;
        config = {
          allowUnfree = true;
          cudaSupport = true;
        };
      };

      rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
        ];
      };

      # CUDA library paths (without ORT - that's added dynamically in shellHook)
      cudaLibPath = pkgs.lib.makeLibraryPath [
        pkgs.cudaPackages.cudatoolkit
        pkgs.cudaPackages.cudnn.lib
        pkgs.stdenv.cc.cc.lib
      ];

      # Base MCP config from mcp-servers-nix (LD_LIBRARY_PATH set dynamically in shellHook)
      mcpConfigBase = mcp-servers-nix.lib.mkConfig pkgs {
        programs = {
          sequential-thinking.enable = true;
          fetch.enable = true;
        };
        settings.servers = {
          rust-code-mcp = {
            command = "./target/release/file-search-mcp";
            args = [ ];
            env = {
              RUST_LOG = "info";
              CUDA_HOME = "${pkgs.cudaPackages.cudatoolkit}";
              CUDA_PATH = "${pkgs.cudaPackages.cudatoolkit}";
            };
          };
        };
      };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        name = "rust-code-mcp";

        buildInputs = with pkgs; [
          # Rust toolchain
          rustToolchain
          rustfmt
          cargo-nextest
          rust-analyzer

          # Build dependencies
          clang
          llvmPackages.bintools
          libclang
          gnumake
          cmake
          pkg-config
          openssl
          sqlite
          protobuf # Required by LanceDB

          # CUDA / GPU (for fastembed ONNX)
          cudaPackages.cudatoolkit
          cudaPackages.cuda_cudart
          cudaPackages.libcublas
          cudaPackages.cudnn

          # Tools for shellHook
          jq
        ];

        LIBCLANG_PATH = "${pkgs.llvmPackages_latest.libclang.lib}/lib";

        CUDA_PATH = "${pkgs.cudaPackages.cudatoolkit}";

        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
          pkgs.cudaPackages.cudatoolkit
          pkgs.cudaPackages.cuda_cudart
          pkgs.cudaPackages.libcublas
          pkgs.cudaPackages.cudnn.lib
          pkgs.stdenv.cc.cc.lib
          pkgs.openssl
        ];

        shellHook = ''
          export CUDA_HOME=${pkgs.cudaPackages.cudatoolkit}
          export PATH=${pkgs.cudaPackages.cudatoolkit}/bin:$PATH

          # Find ORT cache path dynamically (contains libonnxruntime_providers_shared.so)
          ORT_LIB_PATH=$(find "$HOME/.cache/ort.pyke.io/dfbin" -name "libonnxruntime_providers_shared.so" -printf '%h\n' 2>/dev/null | head -1)

          if [ -n "$ORT_LIB_PATH" ]; then
            echo "Found ORT libraries: $ORT_LIB_PATH"
            FULL_LD_PATH="$ORT_LIB_PATH:${cudaLibPath}"
          else
            echo "Warning: ORT cache not found. Run 'cargo build --release' first to download ONNX Runtime."
            echo "CUDA will not work until ORT libraries are cached."
            FULL_LD_PATH="${cudaLibPath}"
          fi

          # Generate .mcp.json with dynamic LD_LIBRARY_PATH
          cat > .mcp.json << EOF
          {
            "mcpServers": {
              "rust-code-mcp": {
                "command": "./target/release/file-search-mcp",
                "args": [],
                "env": {
                  "RUST_LOG": "info",
                  "CUDA_HOME": "${pkgs.cudaPackages.cudatoolkit}",
                  "CUDA_PATH": "${pkgs.cudaPackages.cudatoolkit}",
                  "LD_LIBRARY_PATH": "$FULL_LD_PATH"
                }
              },
              "sequential-thinking": $(cat ${mcpConfigBase} | ${pkgs.jq}/bin/jq '.mcpServers["sequential-thinking"]'),
              "fetch": $(cat ${mcpConfigBase} | ${pkgs.jq}/bin/jq '.mcpServers["fetch"]')
            }
          }
          EOF

          echo "rust-code-mcp dev shell"
          echo "Run 'cargo build --release' to build"
          echo "MCP config generated with CUDA support"
        '';
      };

      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "rust-code-mcp";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;

        nativeBuildInputs = with pkgs; [
          cmake
          pkg-config
          clang
          protobuf # Required by LanceDB
        ];

        buildInputs = with pkgs; [
          openssl
          sqlite
          cudaPackages.cudatoolkit
          cudaPackages.cuda_cudart
          cudaPackages.libcublas
          cudaPackages.cudnn
        ];

        LIBCLANG_PATH = "${pkgs.llvmPackages_latest.libclang.lib}/lib";

        meta = with pkgs.lib; {
          description = "MCP server for semantic Rust code search";
          homepage = "https://github.com/molaco/rust-code-mcp";
          license = licenses.mit;
        };
      };
    };
}
