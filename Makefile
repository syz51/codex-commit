PREFIX ?= $(HOME)/.local
DESTDIR ?=
CARGO_TARGET_DIR ?= target
export CARGO_TARGET_DIR

.PHONY: build release install check lint test

build:
	cargo build --locked --package codex-commit

release:
	cargo build --release --locked --package codex-commit

install: release
	install -d "$(DESTDIR)$(PREFIX)/bin"
	install -m 755 "$(CARGO_TARGET_DIR)/release/codex-commit" "$(DESTDIR)$(PREFIX)/bin/codex-commit"
	install -d "$(DESTDIR)$(PREFIX)/share/zsh/site-functions" "$(DESTDIR)$(PREFIX)/share/bash-completion/completions" "$(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d"
	"$(CARGO_TARGET_DIR)/release/codex-commit" --completions zsh > "$(DESTDIR)$(PREFIX)/share/zsh/site-functions/_codex-commit"
	"$(CARGO_TARGET_DIR)/release/codex-commit" --completions bash > "$(DESTDIR)$(PREFIX)/share/bash-completion/completions/codex-commit"
	"$(CARGO_TARGET_DIR)/release/codex-commit" --completions fish > "$(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/codex-commit.fish"

check:
	cargo fmt --all -- --check
	cargo clippy --workspace --locked --all-targets -- -D warnings

lint: check

test:
	cargo test --workspace --locked
