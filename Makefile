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

build-container-bench:
	sudo docker build -t r2kad-daemon:bench -f daemon/docker/Dockerfile.bench .

build-container-scratch:
	sudo docker build -t r2kad-daemon:bench -f daemon/docker/Dockerfile.bench .

build-containers: build-container-scratch build-container-bench

setup-bench-daemon:
	-sudo docker volume create r2kad-bench-volume

bench-daemon: setup-bench-daemon
	sudo docker compose -f docker-compose-bench.yml up -d
	sleep 1m
	sudo docker compose down
