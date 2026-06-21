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

      # Nix builds must not let ort-sys fetch Pyke's prebuilt ORT archive.
      # `fastembed/ort-download-binaries` gives plain Cargo a self-contained
      # fallback, while Nix points ort-sys at the pinned onnxruntime package and
      # asks it to link dynamically against that package instead.
      ortLibPath = "${pkgs.onnxruntime}/lib";

      # Runtime library path. Mirrors ../nix-devshells/devshells/cuda-code.nix:
      #   - onnxruntime/lib        : ORT shared lib for the local CPU (BGE)
      #                              embedding profile (`local-cpu-small`)
      #   - /run/opengl-driver/lib : NixOS user-space libcuda.so — the GPU
      #                              driver lib, NOT in cudatoolkit; required
      #                              at runtime by any CUDA process
      #   - cudatoolkit / cuda_cudart / libcublas / cudnn : Candle's CUDA deps
      #                              for the local Qwen3 GPU profiles
      ldLibraryPath = pkgs.lib.concatStringsSep ":" [
        ortLibPath
        "/run/opengl-driver/lib"
        "${pkgs.cudaPackages.cudatoolkit}/lib"
        "${pkgs.cudaPackages.cuda_cudart}/lib"
        "${pkgs.cudaPackages.libcublas}/lib"
        "${pkgs.cudaPackages.cudnn.lib}/lib"
        "${pkgs.stdenv.cc.cc.lib}/lib"
      ];

      # Base MCP config (sequential-thinking + fetch helper servers). The
      # rust-code-mcp server entry below carries the CUDA/ORT runtime env so
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
        ORT_LIB_PATH = ortLibPath;
        ORT_PREFER_DYNAMIC_LINK = "1";
        ORT_SKIP_DOWNLOAD = "1";
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
          echo "  CPU build:  cargo build --release"
          echo "  GPU build:  cargo build --release --features cuda"

          # ── Running without a GPU (CPU-only) ───────────────────────────────
          # The default build and automatic embedding profile are CPU-only
          # (`local-cpu-small`, BGE on ONNX/CPU). To use local CUDA/Qwen3
          # profiles, build with `--features cuda` and pass one explicitly:
          #
          #        embedding_profile = "local-gpu-small"
          #        embedding_profile = "local-qwen3-4b"
          #        embedding_profile = "local-qwen3-8b"
          #
          # Without an NVIDIA GPU, use the default profile or pass a CPU/API
          # profile explicitly:
          #        embedding_profile = "local-cpu-small"     (BGE, ONNX on CPU)
          #        embedding_profile = "openrouter-qwen3-8b" (or any OpenRouter
          #                                                   profile; needs
          #                                                   OPENROUTER_API_KEY)
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
        ORT_LIB_PATH = ortLibPath;
        ORT_PREFER_DYNAMIC_LINK = "1";
        ORT_SKIP_DOWNLOAD = "1";

        meta = with pkgs.lib; {
          description = "MCP server for semantic Rust code search";
          homepage = "https://github.com/molaco/rust-code-mcp";
          license = licenses.mit;
        };
      };
    };
}
