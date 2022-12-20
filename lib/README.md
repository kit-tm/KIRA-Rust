# R²/Kad Library

Contains the major building parts for an R²/Kad Daemon implementation.

## Setup

To build the [examples](examples) you have to install Rust.
The recommended way is through [rustup.rs](https://rustup.rs):

```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

This will also install the `cargo` tool which is used for most cli tasks around rust.

## Source Code Organisation

The Domain Layer containing the most basic parts of R²/Kad (e.g. [NodeId](src/domain/node_id.rs)) is located
in [src/domain](src/domain).

The logic around the Domain Layer (called *Use Cases*) is located in the [src/use_cases](src/use_cases) package.

Other packages contained in the [src](src) directory contain Interfaces (called **Traits** in Rust) and some useful
implementations of them to build an implementation of an R²/Kad Routing Tier Application.

Examples are provided in the [examples](examples) directory.

For a more detailed description see the [Source Code Documentation](#Source-Code-Documentation).

## Source Code Documentation

Source Code Documentation can be built with `cargo doc --open` or `make docs`.
Both will open the documentation in your default browser.
