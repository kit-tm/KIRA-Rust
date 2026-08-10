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


default: build

.PHONY: build
build:
	cargo build

$(BIN_PATH):
	cargo build --release

.PHONY: build-release
build-release: # force rebuild of release binary
	cargo build --release


DOCFLAGS = --all-features --open --no-deps
.PHONY: doc
doc: lib-doc r2kad-doc forwarding-doc
%-doc:
	cargo doc --package=kira-$* $(DOCFLAGS)


# Compile m4 files
$(PKG_PREFIX)/%: $(PKG_PREFIX)/%.m4
	m4 -D BIN_DIR=$(BIN_DIR) -D SHARE_DIR=$(SHARE_DIR) $< > $@

.PHONY: install
install: $(PKG_PREFIX)/kirad.service $(PKG_PREFIX)/kirad@.service $(DATA_PREFIX)/nftables.conf $(BIN_PATH)
	$(INSTALL) -m 755 -d $(SYSTEMD_DIR)
	$(INSTALL) -m 644 $(PKG_PREFIX)/kirad.service $(SYSTEMD_DIR)/kirad.service
	$(INSTALL) -m 644 $(PKG_PREFIX)/kirad@.service $(SYSTEMD_DIR)/kirad@.service
	systemctl daemon-reload

	$(INSTALL) -m 755 -d $(SHARE_DIR)
	$(INSTALL) -m 644 $(DATA_PREFIX)/nftables.conf $(SHARE_DIR)/nftables.conf

	$(INSTALL) -m 755 $(BIN_PATH) $(BIN_DIR)

.PHONY: uninstall
uninstall:
	rm $(BIN_DIR)/kirad

	rm $(SYSTEMD_DIR)/kirad.service
	rm $(SYSTEMD_DIR)/kirad@.service
	systemctl daemon-reload

	rm -r $(SHARE_DIR)/


.PHONY: build-image-supervisord build-image-small-k build-image-dns-dht
build-images: build-image-supervisord build-image-small-k build-image-dns-dht

build-image-supervisord:
	docker build -t kira -f docker/Dockerfile.supervisord .

build-image-small-k:
	docker build -t kira-small-k -f docker/Dockerfile.supervisord --build-arg FEATURES="small_buckets,api" .

build-image-dns-dht:
	docker build -t kira-dns-dht examples/dns-4in6-tunnel-example/base


.PHONY: cargo-%
cargo-%:
	@command -v $* >/dev/null || cargo install $*

.PHONY: build-debian-%
build-debian-%: cargo-cross $(PKG_PREFIX)/kirad.service $(PKG_PREFIX)/kirad@.service
	cross build --target $*-unknown-linux-musl --release

.PHONY: pkg-debian-%
pkg-debian-%: build-debian-% cargo-cargo-deb
	cargo deb --target $*-unknown-linux-musl -p kirad --no-build --no-strip


.PHONY: test
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
