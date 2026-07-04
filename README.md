# Library Loader :books:

<!-- ![Screenshot](libloader.png) -->

<!-- Status: [![CircleCI](https://circleci.com/gh/olback/library-loader/tree/master.svg?style=svg)](https://circleci.com/gh/olback/library-loader/tree/master) -->

Status: [![Build Status](https://drone.olback.dev/api/badges/olback/library-loader/status.svg)](https://drone.olback.dev/olback/library-loader)

<!---
OS | Status
-- | ------
Linux | [![CircleCI](https://circleci.com/gh/olback/library-loader/tree/master.svg?style=svg)](https://circleci.com/gh/olback/library-loader/tree/master)
Windows | WIP
Mac | WIP
--->

## Getting started

1. Create an account on [componentsearchengine.com](https://componentsearchengine.com/) if you don't have one already.
2. Download a prebuilt version of library-loader from the [releases page](https://github.com/olback/library-loader/releases) (only linux builds available, see [#67](https://github.com/olback/library-loader/issues/67)).

### Simple install / uninstall

On the [releases page](https://github.com/olback/library-loader/releases), download the latest `library-loader-linux-dist.tar.gz` and untar it. Each release is bundled with two scripts for installing and uninstalling library-loader :

```sh
# Installs both cli/gui binaries in `/usr/bin`
# Installs desktop entry and icon for `library-loader-gui`
sudo install.sh

# Uninstall `library-loader` completely
sudo uninstall.sh
```

### Building from source using Docker

This allows you to build without installing any dependencies on your machine.

```
docker run --volume=$(pwd):/home/circleci/project olback/rust-gtk-linux cargo build --release
```

### Building from source locally(macOS)

Required binaries: brew(from homebrew), rustc, cargo
You have to install rust via rustup and initialize it with rustup-init command.

```shell
./macos-compile.sh
```

### Setup on macOS

Edit the `LibraryLoader.example.toml` and fill in your login details for `componentsearchengine.com`. Rename the file to `LibraryLoader.toml` and place it in `~/Library/Application Support/LibraryLoader.toml`.

e.g.

```shell
cp LibraryLoader.example.toml ~/Library/Application\ Support/LibraryLoader.toml"
```

### Running on macOS

GUI:

```shell
cargo run --bin library-loader-gui
```

or CLI:

```shell
cargo run --bin library-loader-cli
```

### One-shot imports and KiCad 3D models

The CLI can process one downloaded `.epw` file, or a distributor zip containing
an `.epw`, without starting watch mode:

```shell
cargo run --bin library-loader-cli -- --config LibraryLoader.toml --download path/to/model.epw
```

For KiCad libraries, `model_output_path` and `model_uri` can be used to keep 3D
models in a project-level `.3dshapes` directory while rewriting footprint model
references to a KiCad environment variable:

```toml
[formats.'QuartzPulseLib']
format = "kicad"
output_path = "~/Documents/QuartzPulse"
model_output_path = "~/Documents/QuartzPulse/3dmodels/QuartzPulse.3dshapes"
model_uri = "${QP_3DMODELS}"
model_formats = ["stp", "step", "wrl", "stl"]
```

## What/Why?

This is an implementation of [https://www.samacsys.com/library-loader/](https://www.samacsys.com/library-loader/) in Rust. Why? Well, since the library-loader SamacSys provides only works on Windows, I thought it would be neat to make something similar but available to everyone.

For upcomming features, please see the [TODO.md](TODO.md).

## License

[GNU Affero General Public License v3.0](LICENSE)
