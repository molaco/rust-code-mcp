{
  description = "rust-code-mcp - Semantic code search MCP server for Rust codebases";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
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
          protobuf  # Required by LanceDB

          # CUDA / GPU (for fastembed ONNX)
          cudaPackages.cudatoolkit
          cudaPackages.cuda_cudart
          cudaPackages.libcublas
          cudaPackages.cudnn
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
          echo "rust-code-mcp dev shell"
          echo "Run 'cargo build --release' to build"
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
          protobuf  # Required by LanceDB
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
