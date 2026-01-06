# Introduction

This is a Python project managed by [uv] for testing the behaviour of the 
`kirad` Rust daemon in emulated topologies using the [NeST] framework.

# Dependencies

The following tools and dependencies are required to run this project.
This _only_ includes dependencies not managed by [uv].

- [uv]: Python package and project manager
- [iproute] >= 6.9.0 [^1]: Probing setup IPv6-routes by `kirad`.
- Linux based operating system with root privileges:
  [NeST] is utilizing network [namespaces] for emulating the different topology nodes.
  This is a Linux exclusive feature and requires `CAP_SYS_ADMIN`.

# Usage

You can test topologies interactively using a shell called "nesttest".
To start the shell run the following command
while ensuring to have `CAP_SYS_ADMIN` as required by [NeST]:
```sh
uv --project=./tests/kira-test run nesttest
```

You can find preexisting topology files in `./tests/topos` ending with `.gml`.
Instructions on how to use the shell can be aquired by typing `help` inside the shell.

## `kirad` integration tests

The integration tests for `kirad` are located at `kira-test/packages/kira-nest/tests`.
They are using the [pytest] framework.

The required `kirad` binaries are automatically compiled by pytest.
Alternatively you can provide paths to already compiled binaries
using the environment variables `KIRAD_BIN` and `KIRAD_BIN_SMALL-K`.
You can specify a list of special topology files separated by colons
using the `KIRA_TOPOS` environment variable.
Otherwise generic tests will run on all topology files located in `./tests/topos`.

Running the tests can be as simple as just running this command from the root
of the repository:
```sh
uv --project=tests/kira-test run pytest
```

**Note:**
You have to ensure that [pytest] is invoked with `CAP_SYS_ADMIN` which is
commonly done by tools like `sudo` or `run0`.
Using `run0` with systemd version >=259 you could invoke pytest like this.
```sh
run0 \
    --empower \
    --setenv=CARGO_HOME="$CARGO_HOME" \
    --setenv=RUST_WRAPPER="$RUSTC_WRAPPER" \
    --setenv=RUSTUP_HOME="$RUSTUP_HOME" \
    uv --project=tests/kira-test \
    run pytest
```
This will ensure that [pytest] can build the binaries in the same configuration
as the normal developer.

# Development

This project uses the Python project manager [uv].
[uv] automatically provides a Python environment with the required project
dependencies. If you want your editor to pick up the dependencies you should
start your editor with `uv run <EDITOR>`.

For instance, you could start [Neovim] with [Ruff] and [Pyright] setup
as language servers through [`nvim-lspconfig`] and all dependencies will
be correctly picked up by [Ruff] and [Pyright]:

```sh
uv --project=./tests/kira-test run nvim .
```

[uv]: https://docs.astral.sh/uv/
[iproute]: https://wiki.linuxfoundation.org/networking/iproute2
[namespaces]: https://en.wikipedia.org/wiki/Linux_namespaces
[NeST]: https://nest.nitk.ac.in/docs/master/index.html
[pytest]: https://docs.pytest.org/en/stable/index.html
[Ruff]: https://docs.astral.sh/ruff/
[Pyright]: https://github.com/microsoft/pyright
[Neovim]: https://neovim.io/
[`nvim-lspconfig`]: https://github.com/neovim/nvim-lspconfig
[^1]: Prior to version 6.9.0 there is an [issue with the JSON output](https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/commit/?id=0f32ef97babcbe77140a69218917937e6a50fb6c).
