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
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Rust toolchain
            rustToolchain
            cargo-nextest

            # Build dependencies
            pkg-config

            # Tools for end-to-end tests
            nushell
            ripgrep
            sd
            fd
            diffutils
            findutils
            gnused

            # Additional dev tools
            rust-analyzer
          ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };

        apps.end-to-end-test = {
          type = "app";
          program = "${pkgs.writeShellScript "run-end-to-end-test" ''
            set -e
            echo "Building scooter..."
            cargo build --release --locked
            echo "Running scooter end-to-end tests..."
            ${pkgs.nushell}/bin/nu end-to-end-tests/compare-tools.nu
          ''}";
        };
      }
    );
}
