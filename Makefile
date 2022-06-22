.PHONY: setup docs build build-release install

default: build

setup:
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

build:
	cargo build --release

build-no-release:
	cargo build

install-daemon:
	cargo install --path=daemon

docs:
	cargo doc --package r2kad-lib --open

bench:
	cargo bench
