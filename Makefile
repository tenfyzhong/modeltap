.DEFAULT_GOAL := build

.PHONY: build release test check fmt clean

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

check:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --check

clean:
	cargo clean
