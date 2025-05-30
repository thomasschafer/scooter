# End-to-end tests

This directory contains end-to-end tests and benchmarks for Scooter, that compare its behavior against other find and replace tools.

## Prerequisites

### Installing Nix

Install Nix - the Determinate Systems installer can be found [here](https://determinate.systems/nix-installer/), but other methods are available.

## Running tests

From the project root, run:

```bash
nix run .#end-to-end-test
```

## Benchmarks

Benchmarks are run with [hyperfine](https://github.com/sharkdp/hyperfine). To run them yourself:

```sh
nix run .#benchmark
```
