# KIRA Implementation

This repository collects the different repositories used for the implementation of the routing architecture KIRA in Rust started by Moritz
Hepp (2022) at the institute for telematics at KIT.

## Structure

- [KIRA Routing Daemon](kirad): Contains the crate representing the routing daemon executable.
- [KIRA Library](kira-lib): Contains the different abstract modules, classes, traits to implement the routing
  daemon.
- [Examples](examples): Contains minimal examples of running KIRA in an emulated network using [Containernet](https://containernet.github.io)

More specific information can be found in the respective folders and in the following chapters.

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
| Install Daemon             | `make install`                                | Compiles the daemon with release profile and installs it in the system                                            |

### Dependencies

Some of the above tasks require some dependencies to be installed to run them.
Here are the instructions to install them.

- [docker](https://docs.docker.com/get-docker/): To run benchmarks and build docker images. 
Additionaly one has to
  configure the docker daemon to enable ipv6: [like instructed here](https://docs.docker.com/config/daemon/ipv6/).
- [Rust](https://www.rust-lang.org/) 

## Running on physical hosts

To Run the task on a physical host we recommend using the provided
[systemd-service](https://www.man7.org/linux/man-pages/man5/systemd.service.5.html) file.

### Installation

1. `make install`: install the daemon and its files to the system
2. `systemctl daemon-reload`: make systemd pickup newly installed unit files

#### Manage all interfaces

  ```systemctl enable --now kirad.service```

#### Manage some interfaces

Currently, you can only blacklist interfaces using the `kirad@.service` by using the **numbers** of the interfaces:

  ```systemctl enable --now kirad@$(systemd-escape 4,2).service```

This will exclude the interface `4` and `2` interface from being managed by the routing daemon.
You can obtain a list of all your interfaces by running `ip link`.
