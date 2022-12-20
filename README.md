# R²/Kad Implementation

This repository collects the different repositories used for the implementation of R²/Kad in Rust started by Moritz
Hepp (2022) at the institute for telematics at KIT.

## Structure

- [Submodule R²/Kad Routing Daemon](https://git.scc.kit.edu/TM/kira/r2kad-daemon): Contains the crate representing the routing daemon executable.
- [Submodule R²/Kad Library](https://git.scc.kit.edu/TM/kira/r2kad-lib): Contains the different abstract modules, classes, traits to implement the routing
  daemon.

More specific information can be found in the repositories added as submodules and in the following chapters.

## Cloning the repository

To simply clone the repository use this:

```shell
git clone --recurse-submodules git@git.scc.kit.edu:TM/kira/r2kad.git
```

If you want to work on the project and its submodules change the branch in the submodules before changing anything.
Otherwise, some weird version control errors will happen.
E.g.

```shell
cd daemon
git checkout -b main
cd ../lib
git checkout -b main
```

## Tasks

| Task                       | Command                                       | Description                                                                                                       |
|:---------------------------|:----------------------------------------------|:------------------------------------------------------------------------------------------------------------------|
| Build (debug)              | `make`, `cargo build`                         | Compiles the daemon with debug profile and creates an executable                                                  |
| Build (release)            | `make build-release`, `cargo build --release` | Compiles the daemon with release profile and creates an executable                                                |
| Build (docker)             | `make build-images`                           | Compiles and builds scratch and benchmark docker images (no rust install required)                                |
| Build (docker scratch)     | `make build-image-scratch`                    | Compiles and builds scratch docker image (no rust install required)                                               |
| Build (docker bench)       | `make build-image-bench`                      | Compiles and builds benchmark docker image (no rust install required)                                             |
| Install Daemon             | `make install`, `cargo install --path=daemon` | Compiles the daemon with release profile and installs it in the system                                            |
| Create RustDocs            | `make docs`                                   | Creates rustdoc websites for library and daemon from code documentation and opens it in browser (opens two pages) |
| Create Library RustDocs    | `make lib-docs`                               | Creates rustdoc website for the r2kad-lib crate from code documentation and opens it in browser                   |
| Create Daemon RustDocs     | `make daemon-docs`                            | Creates rustdoc website for the r2kad-daemon crate from code documentation and opens it in browser                |
| Run All Tests              | `make test`, `cargo test`                     | Run all tests (unit and integration tests)                                                                        |
| Run Only Unit Tests        | `make unit-tests`, `cargo test --lib`         | Run only unit tests                                                                                               |
| Run Only Integration Tests | `make integration-test`, `cargo test --bins`  | Run only integration tests                                                                                        |
| Run Rust Benchmarks        | `make bench`, `cargo bench`                   | Runs small benchmarks implemented in rust                                                                         |
| Setup Daemon Benchmark     | `make setup-bench-daemon`                     | Creates networks and volumes for docker based benchmarks                                                          |
| Run Daemon Benchmark       | `make bench-daemon`                           | Runs the daemon benchmark (requires Task *Setup Daemon Benchmark* to be run before)                               |

### Dependencies

Some of the above tasks require some dependencies to be installed to run them.
Here are the instructions to install them.

- [GNU/Make](https://www.gnu.org/software/make/#download): Already installed in many Linux distributions. For others see
  the website.
- [docker](https://docs.docker.com/get-docker/): To run benchmarks and build docker images. Additionaly one has to
  configure the docker daemon to enable ipv6: [like instructed here](https://docs.docker.com/config/daemon/ipv6/).
- [docker compose plugin](https://docs.docker.com/compose/install/compose-plugin/): To run the benchmarks.
- [Rust](https://www.rust-lang.org/tools/install): Can be easily installed through `make setup`, which uses curl to
  fetch the installation script as described on the linked website.
