.PHONY: doc-lib build build-release install

default: build

build:
	cargo build --release

build-no-release:
	cargo build

install-daemon:
	cargo install --path=daemon

doc-lib:
	cargo +nightly doc --features unstable-doc-cfg --package r2kad-lib --open
