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

You can test topologies interactively using a shell called _nesttest_.
To start the shell run the following command
```sh
uv --project=./tests/kira-test run nesttest
```

It is recommended to automatically create the directory `/var/run/netns` on boot.
Otherwise, you have to manually create the directory once before running
the nesttest in unshare(2) isolation.
This can be achieved using tmpfiles.d(5):
```sh
printf "d /run/netns 0755 root root -\n" | sudo install -m 0644 /dev/stdin /etc/tmpfiles.d/netns.conf
sudo systemd-tmpfiles --create /etc/tmpfiles.d/netns.conf
```

**Note:** Currently, unshare(2) isolation of nesttest isn't supported if you want to collect OpenTelemetry data
(`--otel` option). You have to ensure to have `CAP_SYS_ADMIN`[^cap_sys_admin] as required by [NeST].

Instructions on how to use the shell can be acquired by typing `help` inside the shell.

## `kirad` integration tests

The integration tests for `kirad` are located at `kira-test/packages/kira-nest/tests`.
They are using the [pytest] framework.

The required `kirad` binaries are automatically compiled by pytest.
Alternatively you can provide paths to already compiled binaries
using the environment variables `KIRAD_BIN` and `KIRAD_BIN_SMALL-K`.
You can specify a list of special topology files separated by colons
using the `KIRA_TOPOS` environment variable.
Otherwise, generic tests will run on all topology files located in [`./tests/topos`].

Running the tests can be as simple as just running this command from the root
of the repository:
```sh
KIRA_TOPOS="tests/topos/minimal.gml" uv --project=tests/kira-test run pytest
```

## Topology files (GML)

Topologies are defined in the [GML format].
You can find preexisting topology files in [`./tests/topos`] ending with `.gml`.

There are several tools provided to generate, alter and show topologies.
All tools are capable of reading from `stdin` and writing to `stdout` as applicable.
For instance, generating a Newman–Watts–Strogatz small-world graph
with at least three connected components, saving it to `random.gml`
while also displaying it to the terminal using `kitten icat`[^kitty]:
```
uv --project=tests/kira-test run generate_random_gml watts -c 3 --seed 42 \
    | tee random.gml \
    | uv --project=tests/kira-test run show_graph \
    | kitten icat
```

Note that `generate_random_gml` by itself doesn't generate _any_ node configs.
If you use the generated topology in _nesttest_, new NodeIds will be
generated on every startup, unless you specify a seed for reproducibility:
```
uv --project=tests/kira-test run generate_random_gml erdos \
    | uv --project=tests/kira-test run nesttest --seed 1234

```

To bake in static NodeIds you can use the `alter_gml_config` utility like this:
```
uv --project=tests/kira-test run generate_random_gml erdos \
    | uv --project=tests/kira-test run alter_gml_config --seed 0000 - randomize > random_backed_config.gml
```

You can use `alter_gml_config` also to remove the configs of all or some nodes
on existing files, or randomize the NodeIds:
```
uv --project=tests/kira-test run alter_gml_config tests/topos/tiny.gml prune 1 > tiny_pruned.gml
uv --project=tests/kira-test run alter_gml_config tests/topos/tiny.gml randomize > tiny_random.gml
```

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
[`./tests/topos`]: ../topos
[GML format]: https://networkx.org/documentation/stable/reference/readwrite/gml.html
[Ruff]: https://docs.astral.sh/ruff/
[Pyright]: https://github.com/microsoft/pyright
[Neovim]: https://neovim.io/
[`nvim-lspconfig`]: https://github.com/neovim/nvim-lspconfig
[^1]: Prior to version 6.9.0 there is an [issue with the JSON output](https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/commit/?id=0f32ef97babcbe77140a69218917937e6a50fb6c).
[^cap_sys_admin]: By using your favorite privilege escalation tool like `sudo`, `run0` or even `run0 --empower` if you don't want to change your user.
[^kitty]: This only works in the [kitty](https://sw.kovidgoyal.net/kitty/) terminal emulator.
          You may have success with [`timg`](https://github.com/hzeller/timg).
