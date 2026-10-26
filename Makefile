PREFIX ?= $(HOME)/.local
DESTDIR ?=

.PHONY: install check lint test

install:
	install -d "$(DESTDIR)$(PREFIX)/bin"
	install -m 755 bin/codex-commit "$(DESTDIR)$(PREFIX)/bin/codex-commit"

check:
	sh -n bin/codex-commit

lint: check
	shellcheck bin/codex-commit

test: check
	uv run --no-project python -B -m unittest discover -s tests -v
