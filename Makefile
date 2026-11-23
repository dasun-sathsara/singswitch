.PHONY: all build fmt fmt-check lint icons check cross bundle-macos install-macos uninstall-macos clean

all: check build

build:
	cargo build --workspace

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

# A missing icon renders as blank space instead of failing the build, so check
# for one here too: `make check` and the bundle path skip scripts/check-macos.sh.
icons:
	./scripts/check-icons.py

check: fmt-check lint icons

cross:
	./scripts/check-macos.sh
	./scripts/check-windows.sh

bundle-macos: icons
	./scripts/build-mac.sh

install-macos:
	./scripts/install-macos.sh

uninstall-macos:
	./scripts/uninstall-macos.sh

clean:
	cargo clean
	rm -rf dist
