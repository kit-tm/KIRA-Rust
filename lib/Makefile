.PHONY: setup docs build build-release install

default: build

setup:
	-curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

docs: setup
	cargo doc --open
