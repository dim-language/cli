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
dim init                      # starting dim.config with a style for every container role
dim check main.dim            # no output when the model is valid
dim check main.dim --sources  # also check every `source` against a local checkout
dim parse main.dim --compact  # JSON model; add --spans for source locations
dim fmt --check               # every .dim file and dim.config under this folder
dim fmt --write
```

`dim check --sources` finds each root's checkout from `--root <id>=<folder>`, or from the git repository that holds the main file when one of its remotes matches the root's `repo`.

Run `dim --help` for the full command list.
