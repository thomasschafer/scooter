{
  description = "scooter - Interactive find and replace TUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix.url = "github:nix-community/fenix";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        rustToolchain = fenix.packages.${system}.stable.toolchain;
        inherit (pkgs.makeRustPlatform fenix.packages.${system}.stable) buildRustPackage;

        testDeps = with pkgs; [
          diffutils
          expect
          fastmod
          fd
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
          cargo-insta
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
            nu tests/e2e-tests.nu test "$@"
          '';
        };

        apps.benchmark = flake-utils.lib.mkApp {
          drv = pkgs.writeShellScriptBin "run-benchmark" ''
            export PATH="${pkgs.lib.makeBinPath benchmarkDeps}:$PATH"
            set -e
            echo "Building..."
            cargo build --release --locked
            echo "Running benchmarks..."
            nu tests/e2e-tests.nu benchmark "$@"
          '';
        };

        packages.default = buildRustPackage {
          pname = "scooter";
          version = "dev";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta.mainProgram = "scooter";
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = shellDeps;
        };

        devShells.benchmark = pkgs.mkShell {
          nativeBuildInputs = shellDeps ++ benchmarkDeps;
        };
      }
    );
}
