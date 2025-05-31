{
  description = "Scooter - Interactive find and replace TUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
        };

        testDeps = with pkgs; [
          diffutils
          expect
          nushell
          ripgrep
          rustToolchain
          sd
        ];

        benchmarkDeps =
          testDeps
          ++ (with pkgs; [
            hyperfine
            rsync
            findutils
          ]);

        shellDeps = with pkgs; [
          rustToolchain
          rust-analyzer
          cargo-watch
          pkg-config
        ];
      in
      {
        apps.end-to-end-test = flake-utils.lib.mkApp {
          drv = pkgs.writeShellScriptBin "run-end-to-end-test" ''
            export PATH="${pkgs.lib.makeBinPath testDeps}:$PATH"
            set -e
            echo "Building..."
            cargo build --release --locked
            echo "Running end-to-end tests..."
            nu tests/compare-tools.nu test
          '';
        };

        apps.benchmark = flake-utils.lib.mkApp {
          drv = pkgs.writeShellScriptBin "run-benchmark" ''
            export PATH="${pkgs.lib.makeBinPath benchmarkDeps}:$PATH"
            set -e
            echo "Building..."
            cargo build --release --locked
            echo "Running benchmarks..."
            nu tests/compare-tools.nu benchmark "$@"
          '';
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = shellDeps;

          shellHook = ''
            export RUST_BACKTRACE=1
          '';
        };

        devShells.benchmark = pkgs.mkShell {
          nativeBuildInputs = shellDeps ++ benchmarkDeps;

          shellHook = ''
            export RUST_BACKTRACE=1
          '';
        };
      }
    );
}
