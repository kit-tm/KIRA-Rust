PKGNAME = r2kad
DATA_PREFIX = daemon/data
PREFIX ?= /usr/local

.PHONY: setup docs build build-release install uninstall

default: build

build:
	cargo build

build-release:
	cargo build --release

install: install-bin install-data

install-bin: target/release/r2kad-daemon
	mkdir -p $(PREFIX)/lib/$(PKGNAME)
	install target/release/r2kad-daemon $(PREFIX)/lib/$(PKGNAME)

install-data: $(DATA_PREFIX)/r2kad.service $(DATA_PREFIX)/nftables.conf
	# todo make vars propagate to service file
	mkdir -p $(PREFIX)/lib/systemd/system
	install $(DATA_PREFIX)/r2kad.service $(PREFIX)/lib/systemd/system
	install $(DATA_PREFIX)/r2kad@.service $(PREFIX)/lib/systemd/system

	mkdir -p $(PREFIX)/share/$(PKGNAME)
	install $(DATA_PREFIX)/nftables.conf $(PREFIX)/share/$(PKGNAME)/nftables.conf

uninstall: uninstall-bin uninstall-data

uninstall-bin:
	rm -r $(PREFIX)/lib/$(PKGNAME)

uninstall-data:
	rm $(PREFIX)/lib/systemd/system/r2kad.service

	rm -r $(PREFIX)/share/$(PKGNAME)

lib-docs:
	cargo doc --package=r2kad-lib --all-features --open

daemon-docs:
	cargo doc --package=r2kad-daemon --all-features --open

docs: lib-docs daemon-docs

unit-test:
	cargo test --lib

integration-test:
	cargo test --bins

test:
	cargo test

build-image-bench:
	sudo docker build -t r2kad-daemon:bench -f daemon/docker/Dockerfile.bench .

build-image-scratch:
	sudo docker build -t r2kad-daemon:scratch -f daemon/docker/Dockerfile.scratch .

build-image-full:
	sudo docker build -t r2kad-daemon:full -f daemon/docker/Dockerfile.full .

build-image-supervisord:
	sudo docker build -t r2kad-daemon:supervisord -f daemon/docker/Dockerfile.supervisord .


build-images: build-image-scratch build-image-bench build-image-full build-image-supervisord build

