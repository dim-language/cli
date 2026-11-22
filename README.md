# Dim CLI

The `dim` command validates Dim diagrams, prints their JSON model, and formats source files.

## Build from source

This repository is the `apps/cli` submodule of the Dim Rust workspace. Building requires access to the private parent repository, which supplies the Rust SDK and workspace dependencies. From the parent workspace, run:

```sh
git submodule update --init apps/cli
cargo install --locked --path apps/cli
```

## Usage

```sh
dim check main.dim
dim parse main.dim --compact
dim fmt main.dim --check
dim fmt main.dim --write
```

Run `dim --help` for the full command list.
