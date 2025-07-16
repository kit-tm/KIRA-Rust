INSTALL ?= install

BIN_NAME = kirad
BIN_PATH = target/release/$(BIN_NAME)

PKGNAME=kira
PKG_PREFIX = pkg
DATA_PREFIX = kirad/conf

PREFIX ?= /usr/local
SYSTEMD_DIR = $(PREFIX)/lib/systemd/system
BIN_DIR = $(PREFIX)/bin
SHARE_DIR = $(PREFIX)/share/$(PKGNAME)


.PHONY: build build-release install uninstall test doc

default: build

build:
	cargo build

build-release:
	cargo build --release


.PHONY: install-bin install-data
install: install-bin install-data

install-bin: build-release
	$(INSTALL) -m 755 $(BIN_PATH) $(BIN_DIR)

$(PKG_PREFIX)/%: $(PKG_PREFIX)/%.m4
	m4 -D BIN_DIR=$(BIN_DIR) -D SHARE_DIR=$(SHARE_DIR) $< > $@

install-data: $(PKG_PREFIX)/kirad.service $(PKG_PREFIX)/kirad@.service $(DATA_PREFIX)/nftables.conf
	mkdir -p $(SYSTEMD_DIR)
	$(INSTALL) -m 644 $(PKG_PREFIX)/kirad.service $(SYSTEMD_DIR)/kirad.service
	$(INSTALL) -m 644 $(PKG_PREFIX)/kirad@.service $(SYSTEMD_DIR)/kirad@.service

	mkdir -p $(SHARE_DIR)
	$(INSTALL) -m 644 $(DATA_PREFIX)/nftables.conf $(SHARE_DIR)/nftables.conf

.PHONY: uninstall-bin uninstall-data
uninstall: uninstall-bin uninstall-data

uninstall-bin:
	rm $(BIN_DIR)/kirad

uninstall-data:
	rm $(SYSTEMD_DIR)/kirad.service
	rm $(SYSTEMD_DIR)/kirad@.service
	rm -r $(SHARE_DIR)/


DOCFLAGS = --all-features --open --no-deps
doc: lib-doc r2kad-doc forwarding-doc
%-doc:
	cargo doc --package=kira-$* $(DOCFLAGS)


test: test-lib test-r2kad test-forwarding 
test-%:
	cargo test --package=kira-$*


.PHONY: jaeger jaeger-clean clean-logs
jaeger:
	docker compose -f tests/jaeger/compose.yaml up --wait

jaeger-clean:
	docker compose -f tests/jaeger/compose.yaml down -v

clean-logs: jaeger-clean
	find -type f -name "k*.log" -delete


.PHONY: build-image-supervisord build-image-small-k build-image-dns-dht
build-images: build-image-supervisord build-image-small-k build-image-dns-dht

build-image-supervisord:
	docker build -t kira -f docker/Dockerfile.supervisord .

build-image-small-k:
	docker build -t kira-small-k -f docker/Dockerfile.supervisord --build-arg FEATURES="small_buckets,api" .

build-image-dns-dht:
	docker build -t kira-dns-dht examples/dns-4in6-tunnel-example/base


cargo-%:
	@command -v $* >/dev/null || cargo install $*

build-debian-%: cargo-cross
	cross build --target $*-unknown-linux-musl --release

pkg-debian-%: build-debian-% cargo-cargo-deb
	cargo deb --target $*-unknown-linux-musl -p kirad --no-build
