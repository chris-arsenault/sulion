.PHONY: ci lint-rust fmt-rust test-rust test-rust-integration lint-ts typecheck-ts test-ts e2e e2e-install

ci: lint-rust fmt-rust test-rust test-rust-integration lint-ts typecheck-ts test-ts

lint-rust:
	cd backend && cargo clippy --release -- -D warnings -W clippy::cognitive_complexity
	cd backend && cargo test --release --test structure_lint

fmt-rust:
	cd backend && cargo fmt -- --check

test-rust:
	cd backend && cargo test --release --lib --bins
	cd backend && cargo test --release --doc

test-rust-integration:
	./scripts/run-backend-integration-tests.sh

lint-ts:
	cd frontend && pnpm exec eslint .

typecheck-ts:
	cd frontend && pnpm exec tsc --noEmit

test-ts:
	cd frontend && pnpm exec vitest run

e2e:
	cd frontend && pnpm exec playwright test

e2e-install:
	cd frontend && pnpm exec playwright install chromium
