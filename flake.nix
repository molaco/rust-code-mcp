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

      # Runtime library path. Mirrors ../nix-devshells/devshells/cuda-code.nix:
      #   - onnxruntime/lib        : ORT shared lib for the local CPU (BGE)
      #                              embedding profile (`local-cpu-small`)
      #   - /run/opengl-driver/lib : NixOS user-space libcuda.so — the GPU
      #                              driver lib, NOT in cudatoolkit; required
      #                              at runtime by any CUDA process
      #   - cudatoolkit / cuda_cudart / libcublas / cudnn : Candle's CUDA deps
      #                              for the local Qwen3 GPU profiles
      ldLibraryPath = pkgs.lib.concatStringsSep ":" [
        "${pkgs.onnxruntime}/lib"
        "/run/opengl-driver/lib"
        "${pkgs.cudaPackages.cudatoolkit}/lib"
        "${pkgs.cudaPackages.cuda_cudart}/lib"
        "${pkgs.cudaPackages.libcublas}/lib"
        "${pkgs.cudaPackages.cudnn.lib}/lib"
        "${pkgs.stdenv.cc.cc.lib}/lib"
      ];

      # fastembed's `ort-load-dynamic` feature dlopens ONNX Runtime at this path.
      ortDylibPath = "${pkgs.onnxruntime}/lib/libonnxruntime.so";

      # Base MCP config (sequential-thinking + fetch helper servers). The
      # rust-code-mcp server entry below carries the CUDA/ONNX runtime env so
      # Claude Code can spawn it directly.
      mcpConfigBase = mcp-servers-nix.lib.mkConfig pkgs {
        programs = {
          sequential-thinking.enable = true;
          fetch.enable = true;
        };
        settings.servers = {
          rust-code-mcp = {
            command = "./target/release/rust-code-mcp";
            args = [ ];
            env = {
              RUST_LOG = "info";
              CUDA_HOME = "${pkgs.cudaPackages.cudatoolkit}";
              CUDA_PATH = "${pkgs.cudaPackages.cudatoolkit}";
              ORT_DYLIB_PATH = ortDylibPath;
              LD_LIBRARY_PATH = ldLibraryPath;
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
          protobuf # required by LanceDB
          mold # fast linker

          # ONNX Runtime — backs the local CPU embedding profile (BGE).
          onnxruntime

          # CUDA / GPU — backs the default local Qwen3 (Candle) profiles.
          # cudatoolkit also provides nvcc for cudarc's build script.
          cudaPackages.cudatoolkit
          cudaPackages.cuda_cudart
          cudaPackages.libcublas
          cudaPackages.cudnn

          # Tools for shellHook
          jq
        ];

        LIBCLANG_PATH = "${pkgs.llvmPackages_latest.libclang.lib}/lib";
        CUDA_HOME = "${pkgs.cudaPackages.cudatoolkit}";
        CUDA_PATH = "${pkgs.cudaPackages.cudatoolkit}";
        ORT_DYLIB_PATH = ortDylibPath;
        LD_LIBRARY_PATH = ldLibraryPath;

        shellHook = ''
          # nvcc on PATH for cudarc's build script.
          export PATH="${pkgs.cudaPackages.cudatoolkit}/bin:$PATH"

          # Generate .mcp.json (gitignored) so Claude Code can spawn the
          # server with the CUDA/ONNX runtime environment baked in. Only
          # written if absent, so a hand-edited config is never clobbered.
          if [ ! -f .mcp.json ]; then
            cat > .mcp.json <<EOF
          {
            "mcpServers": {
              "rust-code-mcp": {
                "command": "./target/release/rust-code-mcp",
                "args": [],
                "env": {
                  "RUST_LOG": "info",
                  "CUDA_HOME": "${pkgs.cudaPackages.cudatoolkit}",
                  "CUDA_PATH": "${pkgs.cudaPackages.cudatoolkit}",
                  "ORT_DYLIB_PATH": "${ortDylibPath}",
                  "LD_LIBRARY_PATH": "${ldLibraryPath}"
                }
              },
              "sequential-thinking": $(${pkgs.jq}/bin/jq '.mcpServers["sequential-thinking"]' ${mcpConfigBase}),
              "fetch": $(${pkgs.jq}/bin/jq '.mcpServers["fetch"]' ${mcpConfigBase})
            }
          }
          EOF
            echo ".mcp.json generated."
          else
            echo ".mcp.json already exists; leaving it untouched."
          fi

          echo "rust-code-mcp dev shell (CUDA + ONNX)"
          echo "  build:  cargo build --release   ->   ./target/release/rust-code-mcp"

          # ── Running without a GPU (CPU-only) ───────────────────────────────
          # The default embedding profile is GPU (Qwen3 on Candle/CUDA). To run
          # without an NVIDIA GPU, neither of these needs CUDA at run time:
          #
          #   1. Keep this (GPU-capable) build, but index/search with a CPU or
          #      API profile — the GPU code paths are simply never exercised:
          #        embedding_profile = "local-cpu-small"     (BGE, ONNX on CPU)
          #        embedding_profile = "openrouter-qwen3-8b" (or any OpenRouter
          #                                                   profile; needs
          #                                                   OPENROUTER_API_KEY)
          #
          #   2. For a machine with NO CUDA toolkit at all, produce a fully
          #      CPU-only *build* by removing the `cuda` feature from the
          #      `fastembed` dependency in Cargo.toml. The `local-gpu-*` /
          #      `local-qwen3-*` profiles are then unavailable, but the ONNX
          #      and OpenRouter profiles work and the build no longer needs
          #      nvcc or the CUDA libraries above.
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
          protobuf # required by LanceDB
        ];

        buildInputs = with pkgs; [
          openssl
          sqlite
          onnxruntime
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
