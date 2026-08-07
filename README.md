# KIRA Implementation

This repository collects the different repositories used for the implementation
of the scalable zero-touch routing architecture [KIRA](https://s.kit.edu/KIRA)
in Rust started by Moritz Hepp (2022) at the [Institute of Telematics](https://telematics.tm.kit.edu)
at [KIT](https://www.kit.edu/). This implementation supplies a routing daemon
that provides IPv6 connectivity without configuration as well as a distributed
hash table that can be used to provide a simple key-value store to map names
to IPv6 addresses. The IPv6 addresses are currently randomly generated from
the [ULA](https://datatracker.ietf.org/doc/html/rfc4193) address realm and
are as such not routable on the Internet. As KIRA is designed to be a routing
solution for control planes it deliberately uses ULAs for now.

## Implementation Status
**This implementation is in version 0.0.0 (pre-MVP) and therefore is still work-in-progress in many parts.**

For a working example see [DNS-DHT-Example](./examples/dns-4in6-tunnel-example/).

## Structure

- [KIRA Routing Daemon](kirad): Contains the crate representing the routing daemon executable.
- [Sans-I/O R²/KAD](kira-r2kad): Contains the implementation of the routing protocol of KIRA R²/KAD.
- [KIRA Library](kira-lib): Contains the different abstract modules, classes, traits
  to implement a complete routing daemon.
  Crucially this provides the i/o implementation of R²/KAD.
- [KIRA Forwarding](kira-forwarding): Contains the traits and implementations of the fast forwarding layer of KIRA.
- [Examples](examples): Contains minimal examples of running KIRA in an emulated network using [Containernet](https://containernet.github.io)

More specific information can be found in the respective folders and in the following chapters.

## Rustdoc

The majority of the KIRA routing daemon is written in Rust.
To access the [rustdoc](https://doc.rust-lang.org/rustdoc/index.html) of the respective packages run
```shell
make doc
```

## Cloning the repository

To simply clone the repository use this:

```shell
git clone git@gitlab.kit.edu:kit/tm/telematics/kira/kira-rust.git
```

## Tasks

| Task                       | Command                                       | Description                                                                                                       |
|:---------------------------|:----------------------------------------------|:------------------------------------------------------------------------------------------------------------------|
| Build (release)            | `make build-release`, `cargo build --release` | Compiles the daemon with release profile and creates an executable                                                |
| Build (docker)             | `make build-images`                           | Compiles and builds scratch and benchmark docker images (no rust install required)                                |
| Install Daemon             | `sudo make install`                           | Compiles the daemon with release profile and installs it in the system                                            |

### Dependencies

Some of the above tasks require some dependencies to be installed to run them.
Here are the instructions to install them.

- [docker](https://docs.docker.com/get-docker/): To run benchmarks and build docker images.
  Additionaly one has to [configure the docker daemon to support IPv6](https://docs.docker.com/config/daemon/ipv6/).
- [Rust](https://www.rust-lang.org/)
- _Userspace_ utilities of [nftables](https://wiki.nftables.org/wiki-nftables/index.php/Main_Page#Installing_nftables).
  Specifically the daemon must have access to the `nft` utility.

## Running on physical hosts

To Run the task on a physical host we recommend using the provided
[systemd-service](https://www.man7.org/linux/man-pages/man5/systemd.service.5.html) file.

### Installation

1. `make build-release`: Build the release version of `kirad`
2. `sudo make install`: install the daemon and its files to the system

#### Manage all interfaces

  ```systemctl enable --now kirad.service```

#### Manage some interfaces

Currently, you can only blacklist interfaces using the `kirad@.service` by using the **numbers** of the interfaces:

  ```systemctl enable --now kirad@$(systemd-escape 4,2).service```

This will exclude the interface `4` and `2` interface from being managed by the routing daemon.
You can obtain a list of all your interfaces by running `ip link`.

## Running docker test scenarios
Check [tests/README.md](/tests/README.md) for further information.
