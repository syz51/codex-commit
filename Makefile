PREFIX ?= $(HOME)/.local
DESTDIR ?=
CARGO_TARGET_DIR ?= target
export CARGO_TARGET_DIR

.PHONY: build release install check lint test

build:
	cargo build --locked

release:
	cargo build --release --locked

install: release
	install -d "$(DESTDIR)$(PREFIX)/bin"
	install -m 755 "$(CARGO_TARGET_DIR)/release/codex-commit" "$(DESTDIR)$(PREFIX)/bin/codex-commit"

check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets -- -D warnings

lint: check

test: build
	cargo test --locked
	CODEX_COMMIT_BIN="$(abspath $(CARGO_TARGET_DIR))/debug/codex-commit" uv run --no-project python -B -m unittest discover -s tests -v
