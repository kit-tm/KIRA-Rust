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
	sudo docker build --no-cache -t r2kad-daemon:bench -f daemon/docker/Dockerfile.bench .

build-container-scratch:
	sudo docker build --no-cache -t r2kad-daemon:bench -f daemon/docker/Dockerfile.bench .

build-containers: build-container-scratch build-container-bench

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
	# 1. iteration
	sudo docker compose -f docker-compose-bench.yml up -d
	sleep 1m
	sudo docker compose -f docker-compose-bench.yml stop node-3
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml start node-3
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml down
	# 2. Iteration
	sudo docker compose -f docker-compose-bench.yml up -d
	sleep 1m
	sudo docker compose -f docker-compose-bench.yml stop node-7
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml start node-7
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml down
	# 3. Iteration
	sudo docker compose -f docker-compose-bench.yml up -d
	sleep 1m
	sudo docker compose -f docker-compose-bench.yml stop node-10
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml start node-10
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml down
	# 4. Iteration
	sudo docker compose -f docker-compose-bench.yml up -d
	sleep 1m
	sudo docker compose -f docker-compose-bench.yml down node-4
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml start node-4
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml down
	# 5. Iteration
	sudo docker compose -f docker-compose-bench.yml up -d
	sleep 1m
	sudo docker compose -f docker-compose-bench.yml stop node-8
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml start node-8
	sleep 2m
	sudo docker compose -f docker-compose-bench.yml  down
