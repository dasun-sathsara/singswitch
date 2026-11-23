.PHONY: all build fmt fmt-check lint check cross bundle-macos install-macos uninstall-macos clean

all: check build

build:
	cargo build --workspace

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

check: fmt-check lint

cross:
	./scripts/check-macos.sh
	./scripts/check-windows.sh

bundle-macos:
	./scripts/build-mac.sh

install-macos:
	./scripts/install-macos.sh

uninstall-macos:
	./scripts/uninstall-macos.sh

clean:
	cargo clean
	rm -rf dist
