.DEFAULT_GOAL := build

.PHONY: build release test bench check fmt clean

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

bench:
	cargo bench

check:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --check

clean:
	cargo clean
