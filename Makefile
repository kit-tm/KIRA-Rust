PKGNAME = kira
DATA_PREFIX = kirad/conf
PKG_PREFIX = pkg
PREFIX ?= /usr/local

.PHONY: setup docs build build-release install uninstall

default: build

build:
	cargo build

build-release:
	cargo build --release

install: install-bin install-data

install-bin: build-release
	install target/release/kirad $(PREFIX)/bin/

install-data: $(PKG_PREFIX)/kirad.service $(DATA_PREFIX)/nftables.conf
	# todo make vars propagate to service file
	mkdir -p $(PREFIX)/lib/systemd/system
	install $(PKG_PREFIX)/kirad.service $(PREFIX)/lib/systemd/system
	install $(PKG_PREFIX)/kirad@.service $(PREFIX)/lib/systemd/system

	mkdir -p $(PREFIX)/share/$(PKGNAME)
	install $(DATA_PREFIX)/nftables.conf $(PREFIX)/share/$(PKGNAME)/nftables.conf

uninstall: uninstall-bin uninstall-data

uninstall-bin:
	rm $(PREFIX)/bin/kirad

uninstall-data:
	rm $(PREFIX)/lib/systemd/system/kirad.service
	rm $(PREFIX)/lib/systemd/system/kirad@.service

	rm -r $(PREFIX)/share/$(PKGNAME)

lib-docs:
	cargo doc --package=kira-lib --all-features --open

daemon-docs:
	cargo doc --package=kirad --all-features --open

docs: lib-docs daemon-docs

unit-test:
	cargo test --lib

integration-test:
	cargo test --bins

test:
	cargo test

build-image-bench:
	docker build -t kira:bench -f docker/Dockerfile.bench .

build-image-scratch:
	docker build -t kira:scratch -f docker/Dockerfile.scratch .

build-image-full:
	docker build -t kira:full -f docker/Dockerfile.full .

build-image-supervisord:
	docker build -t kira -f docker/Dockerfile.supervisord .

build-image-small-k:
	docker build -t kira-small-k -f docker/Dockerfile.small-k .

build-images: build-image-scratch build-image-bench build-image-full build-image-supervisord build

build-debian-x86:
	cross build --target x86_64-unknown-linux-musl --release

pkg-debian-x86: build-debian-x86
	cargo deb --target x86_64-unknown-linux-musl -p kirad --no-build
	
build-debian-aarch64:
	cross build --target x86_64-unknown-linux-musl --release

pkg-debian-aarch64: build-debian-aarch64
	cargo deb --target x86_64-unknown-linux-musl -p kirad --no-build --no-strip
