# KIRA Implementation

This repository collects the different repositories used for the implementation
of the scalable zero-touch routing architecture [KIRA] in Rust started by
Moritz Hepp (2022) at the [Institute of Telematics] at [KIT].
This implementation supplies a routing daemon that provides IPv6 connectivity
without configuration as well as a [distributed hash table] that can be used to
provide a simple key-value store to map names to IPv6 addresses.
The IPv6 addresses are currently randomly generated from the [ULA] address realm
and are as such _not routable_ on the Internet.
As [KIRA] is designed to be a routing solution for control planes it deliberately
uses [ULA]s for now.

## Implementation Status
**This implementation is in version 0.0.0 (pre-MVP) and therefore is still work-in-progress in many parts.**

For a working example see [DNS-DHT-Example](./examples/dns-4in6-tunnel-example/).

## Repo Structure

- [KIRA Routing Daemon](kirad):
  Contains the crate representing the routing daemon executable.
- [Sans-I/O R²/KAD](kira-r2kad):
  Contains the implementation of the routing protocol of [KIRA], R²/KAD.
- [KIRA Library](kira-lib):
  Contains the different abstract modules, classes, traits to implement a complete
  routing daemon. Crucially this provides the i/o implementation of R²/KAD.
- [KIRA Forwarding](kira-forwarding):
  Contains the traits and implementations of the fast forwarding layer of KIRA.
- [Examples](examples):
  Contains minimal examples of running KIRA in an emulated network using [Containernet].

More specific information can be found in the respective folders and in the following chapters.

## Documentation

The [KIRA] routing daemon is primarily implemented in Rust,
leveraging the language's documentation capabilities through [rustdoc].
To generate and view the documentation for the respective packages, run:
```sh
make doc
```

## Cloning the repository

To simply clone the repository use this:
```sh
git clone git@gitlab.kit.edu:kit/tm/telematics/kira/kira-rust.git
```

## Tasks

| Task                       | Command                                       | Description                                                                                                       |
|:---------------------------|:----------------------------------------------|:------------------------------------------------------------------------------------------------------------------|
| Build                      | `make build`, `cargo build`                   | Compiles the daemon with creates an executable                                                                    |
| Build (release)            | `make build-release`, `cargo build --release` | Compiles the daemon with release profile and creates an executable[^prod_executable_path]                         |
| Build (docker)             | `make build-images`                           | Compiles and builds provided docker images (no Rust install required)                                             |
| Code Documentation         | `make doc`                                    | Generates and views the documentation of all Rust packages                                                        |
| Install Daemon             | `make install`                                | Compiles the daemon with release profile and installs it system-wide                                              |
| Uninstall Daemon           | `make uninstall`                              | Removes the daemon installation from the system                                                                   |
| Debian Package             | `make pkg-debian-<TARGET>`                    | Build a Debian package for the target architecture[^arch] (targets: `x86_64`, `aarch64`)                          |

### Dependencies

- [Rust](https://www.rust-lang.org/):
  Build the daemon and documentation from source (not required for building the container images),
  a full Rust toolchain is recommended.
- [Docker](https://docs.docker.com/get-docker/):
  Run examples, build container images, cross compile Debian packages.
- [m4](https://pkgs.org/search/?q=%2Fusr%2Fbin%2Fm4): Install daemon, build Debian packages.
- [dpkg-shlibdeps](https://pkgs.org/search/?q=%2Fusr%2Fbin%2Fdpkg-shlibdeps):
  Build Debian packages.


### Runtime Dependencies

- _Userspace_ utilities of [nftables](https://wiki.nftables.org/wiki-nftables/index.php/Main_Page#Installing_nftables):
  The daemon must have access to the `nft` utility to load the [`nftables.conf`](kirad/conf/nftables.conf).

## Running on physical hosts

To run the task on a physical host we recommend using the provided
[systemd-service](https://www.man7.org/linux/man-pages/man5/systemd.service.5.html) files.

### Installation

1. `make build-release`: Build the release version of `kirad`
2. `sudo make install`: install the daemon and its files to the system

If you're on a Debian-based system (dpkg), you can also build and install
the Debian package of your architecture (example on `x86_64`):
```sh
make pkg-debian-x86_64
sudo apt-get install target/x86_64-unknown-linux-musl/debian/kirad_0.1.0-1_amd64.deb
```

#### Manage All Interfaces

```sh
systemctl enable --now kirad.service
```

#### Manage Some Interfaces

Currently, you can only blacklist interfaces using the `kirad@.service` by using the **numbers** of the interfaces:

```sh
systemctl enable --now kirad@$(systemd-escape 4,2).service
```

This will exclude the interface `4` and `2` interface from being managed by the routing daemon.
You can obtain a list of all your interfaces by running `ip link`.

## Development

You should install the following additional dependencies for development:

- [pre-commit](https://pre-commit.com/#3-install-the-git-hook-scripts):
  Catch invalid commits that would be flagged later by GitLab's CI.
- [Clippy](https://github.com/rust-lang/rust-clippy#step-2-install-clippy):
  Catch common mistakes in Rust code.
- [Nightly `rustfmt`](https://github.com/rust-lang/rustfmt#on-the-nightly-toolchain):
  make sure your Rust code conforms the set style guide.
- [rust-analyzer](https://rust-analyzer.github.io/book/installation.html):
  Language server for your favorite IDE (optional).

If you want to contribute to the Python-based emulation powered by [NeST]
at [`kira-test`](tests/kira-test), you should additionally install the used
project manager [uv](https://docs.astral.sh/uv/getting-started/installation/).

### Testing

You should test your changes before submitting any changes.
Write Rust unit tests and run them using:
```sh
cargo test --workspace
```

Additionally, you can test the `kirad` in network emulation scenarios
interactively using the _nesttest_ [REPL] or run automatic tests on the provided topology:
```sh
uv --project=./tests/kira-test run nesttest tests/topos/minimal.gml
KIRA_TOPOS="tests/topos/minimal.gml" uv --project=tests/kira-test run pytest
```
Since the size of the emulated topologies is rather small, you should
probably build `kirad` with the `small_buckets` feature
to store less contacts per bucket of the routing table.
Pytest is doing this automatically but for _nesttest_ you have to build
a binary manually:
```sh
cargo build --features=small_buckets
```
More information on the Python-based emulation can be found at [`kira-test`](tests/kira-test).


[KIRA]: https://s.kit.edu/KIRA
[Institute of Telematics]: https://telematics.tm.kit.edu
[KIT]: https://www.kit.edu/
[distributed hash table]: https://en.wikipedia.org/wiki/Distributed_hash_table
[ULA]: https://datatracker.ietf.org/doc/html/rfc4193
[rustdoc]: https://doc.rust-lang.org/rustdoc/index.html
[Containernet]: https://containernet.github.io
[cargo-cross]: https://github.com/cross-rs/cross
[Docker]: https://www.docker.com/
[Podman]: https://podman.io/
[NeST]: https://gitlab.com/nitk-nest/nest
[REPL]: https://en.wikipedia.org/wiki/Read%E2%80%93eval%E2%80%93print_loop

[^prod_executable_path]: The executable can be found at `target/release/kirad`
[^arch]: `<TARGET>-unknown-linux-musl`, cross compilation is done using [cargo-cross] and requires [Docker] or [Podman].
