# End-to-end tests

This directory contains end-to-end tests for Scooter that compare its behavior against other find and replace tools.

## Prerequisites

### Installing Nix

Install Nix - the Determinate Systems installer can be found [here](https://determinate.systems/nix-installer/), but other methods are available.

## Running tests

From the project root, run:

```bash
nix run .#end-to-end-test
```

## Test structure

The `compare-tools.nu` test:
1. Creates a `test-input/` directory with a number of test files
1. Runs Scooter and other tools on copies of this directory
1. Compares the outputs to ensure Scooter produces identical results
1. Cleans up all test directories
