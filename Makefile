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
	sudo install target/release/kirad $(PREFIX)/bin/

install-data: $(PKG_PREFIX)/kirad.service $(DATA_PREFIX)/nftables.conf
	# todo make vars propagate to service file
	sudo mkdir -p $(PREFIX)/lib/systemd/system
	sudo install $(PKG_PREFIX)/kirad.service $(PREFIX)/lib/systemd/system
	sudo install $(PKG_PREFIX)/kirad@.service $(PREFIX)/lib/systemd/system

	sudo mkdir -p $(PREFIX)/share/$(PKGNAME)
	sudo install $(DATA_PREFIX)/nftables.conf $(PREFIX)/share/$(PKGNAME)/nftables.conf

uninstall: uninstall-bin uninstall-data

uninstall-bin:
	rm $(PREFIX)/bin/kirad

uninstall-data:
	rm $(PREFIX)/lib/systemd/system/kirad.service
	rm $(PREFIX)/lib/systemd/system/kirad@.service

	rm -r $(PREFIX)/share/$(PKGNAME)

lib-docs:
	cargo doc --package=kira-lib --all-features --open --no-deps
r2kad-docs:
	cargo doc --package=kira-r2kad --all-features --open --no-deps
forwarding-docs:
	cargo doc --package=kira-forwarding --all-features --open --no-deps

docs: lib-docs r2kad-docs forwarding-docs

unit-test:
	cargo test --lib

integration-test:
	cargo test --bins

test:
	cargo test

jaeger:
	docker compose -f tests/jaeger-tracing-compose.yml up --wait

jaeger-clean:
	docker compose -f tests/jaeger-tracing-compose.yml down -v

clean-logs: jaeger-clean
	find -type f -name "k*.log" -delete

build-image-supervisord:
	docker build -t kira -f docker/Dockerfile.supervisord .

build-image-small-k:
	docker build -t kira-small-k -f docker/Dockerfile.small-k .

build-images: build-image-supervisord build-image-small-k

build-debian-x86:
	cross build --target x86_64-unknown-linux-musl --release

pkg-debian-x86: build-debian-x86
	cargo deb --target x86_64-unknown-linux-musl -p kirad --no-build

build-debian-aarch64:
	cross build --target aarch64-unknown-linux-musl --release

pkg-debian-aarch64: build-debian-aarch64
	cargo deb --target aarch64-unknown-linux-musl -p kirad --no-build --no-strip
