.PHONY: test lint

test: lint
	node --test tests/releases.test.cjs
	python3 tests/integration_test.py

lint:
	node --check tests/releases.test.cjs
	git diff --check -- .
