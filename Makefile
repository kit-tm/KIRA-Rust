.PHONY: doc-lib build build-release install

default: build

build:
	cargo build --release

build-no-release:
	cargo build

install-daemon:
	cargo install --path=daemon

doc-lib:
	cargo doc --package r2kad-lib --open

bench:
	cargo bench
