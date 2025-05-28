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
      in
      {
        apps.end-to-end-test = flake-utils.lib.mkApp {
          drv = pkgs.writeShellScriptBin "run-end-to-end-test" ''
            export PATH="${pkgs.lib.makeBinPath testDeps}:$PATH"
            set -e
            echo "Building scooter..."
            cargo build --release --locked
            echo "Running scooter end-to-end tests..."
            nu end-to-end-tests/compare-tools.nu
          '';
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            rustToolchain
            rust-analyzer
            cargo-watch
            pkg-config
          ];

          shellHook = ''
            export RUST_BACKTRACE=1
          '';
        };
      }
    );
}
