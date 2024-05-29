PKGNAME = r2kad
DATA_PREFIX = daemon/data
PREFIX ?= /usr/local

.PHONY: setup docs build build-release install uninstall

default: build

setup:
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

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

bench:
	cargo bench

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

build-debian-x86:
	cross build --target x86_64-unknown-linux-musl --release

pkg-debian-x86: build-debian-x86
	cargo deb --target x86_64-unknown-linux-musl -p kirad --no-build
	
build-debian-aarch64:
	cross build --target x86_64-unknown-linux-musl --release

pkg-debian-aarch64: build-debian-aarch64
	cargo deb --target x86_64-unknown-linux-musl -p kirad --no-build --nostrip

setup-bench-daemon:
	-sudo docker volume create r2kad-bench-volume
	-sudo docker network create --ipv6 --subnet="2001:db8:1::/64" --gateway="2001:db8:1::1" mynetv6-1
	-sudo docker network create --ipv6 --subnet="2001:db8:2::/64" --gateway="2001:db8:2::1" mynetv6-2
	-sudo docker network create --ipv6 --subnet="2001:db8:3::/64" --gateway="2001:db8:3::1" mynetv6-3
	-sudo docker network create --ipv6 --subnet="2001:db8:4::/64" --gateway="2001:db8:4::1" mynetv6-4
	-sudo docker network create --ipv6 --subnet="2001:db8:5::/64" --gateway="2001:db8:5::1" mynetv6-5
	-sudo docker network create --ipv6 --subnet="2001:db8:6::/64" --gateway="2001:db8:6::1" mynetv6-6
	-sudo docker network create --ipv6 --subnet="2001:db8:7::/64" --gateway="2001:db8:7::1" mynetv6-7
	-sudo docker network create --ipv6 --subnet="2001:db8:8::/64" --gateway="2001:db8:8::1" mynetv6-8
	-sudo docker network create --ipv6 --subnet="2001:db8:9::/64" --gateway="2001:db8:9::1" mynetv6-9

bench-daemon:
	# 10 Iterationen der Benchmark
	for NUMBER in 1 2 3 4 5 6 7 8 9 10 ; do \
  		echo "Iteration #"$$NUMBER; \
        sudo docker compose -f docker-compose-bench.yml up -d ; \
        sleep 1m ; \
        sudo docker compose -f docker-compose-bench.yml stop node-3 ; \
        sleep 2m ; \
        sudo docker compose -f docker-compose-bench.yml start node-3 ; \
        sleep 2m ; \
        sudo docker compose -f docker-compose-bench.yml down ; \
    done
