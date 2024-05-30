PKGNAME = kira
DATA_PREFIX = kirad/conf
PREFIX ?= /usr/local

.PHONY: setup docs build build-release install uninstall

default: build

build:
	cargo build

build-release:
	cargo build --release

install: install-bin install-data

install-bin: target/release/kirad
	mkdir -p $(PREFIX)/lib/$(PKGNAME)
	install target/release/kirad $(PREFIX)/lib/$(PKGNAME)

install-data: $(DATA_PREFIX)/kira.service $(DATA_PREFIX)/nftables.conf
	# todo make vars propagate to service file
	mkdir -p $(PREFIX)/lib/systemd/system
	install $(DATA_PREFIX)/kira.service $(PREFIX)/lib/systemd/system
	install $(DATA_PREFIX)/kira@.service $(PREFIX)/lib/systemd/system

	mkdir -p $(PREFIX)/share/$(PKGNAME)
	install $(DATA_PREFIX)/nftables.conf $(PREFIX)/share/$(PKGNAME)/nftables.conf

uninstall: uninstall-bin uninstall-data

uninstall-bin:
	rm -r $(PREFIX)/lib/$(PKGNAME)

uninstall-data:
	rm $(PREFIX)/lib/systemd/system/kira.service

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
	docker build -t kira:supervisord -f docker/Dockerfile.supervisord .


build-images: build-image-scratch build-image-bench build-image-full build-image-supervisord build

