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

TODO: Add instructions on how to use the project.

# Development

This project uses the Python project manager [uv].
[uv] automatically provides a Python environment with the required project
dependencies. If you want your editor to pick up the dependencies you should
start your editor with `uv run <EDITOR>`.

For instance, you could start [Neovim] with [Ruff] and [Pyright] setup
as language servers through [`nvim-lspconfig`] and all dependencies will
be correctly picked up by [Ruff] and [Pyright]:

```sh
cd tests/kira-test
uv run nvim .
```

[^1]: Prior to version 6.9.0 there is an [issue with the JSON output](https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/commit/?id=0f32ef97babcbe77140a69218917937e6a50fb6c).
[uv]: https://docs.astral.sh/uv/
[iproute]: https://wiki.linuxfoundation.org/networking/iproute2
[namespaces]: https://en.wikipedia.org/wiki/Linux_namespaces
[NeST]: https://nest.nitk.ac.in/docs/master/index.html
[Ruff]: https://docs.astral.sh/ruff/
[Pyright]: https://github.com/microsoft/pyright
[Neovim]: https://neovim.io/
[`nvim-lspconfig`]: https://github.com/neovim/nvim-lspconfig
