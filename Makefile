.PHONY: ci validate-deploy validate-nix test-nix lint-rust fmt-rust test-rust test-rust-integration lint-ts typecheck-ts test-ts e2e e2e-install screenshots

ci: validate-deploy lint-rust fmt-rust test-rust lint-ts typecheck-ts test-ts

validate-deploy:
	docker compose --env-file deploy/compose.validate.env -f compose.yaml config --quiet
	docker compose --env-file deploy/compose.validate.env -f deploy/compose.truenas-standalone.yaml config --quiet
	docker compose --env-file deploy/compose.validate.env -f compose.yaml -f deploy/compose.standalone.yaml config --quiet
	docker compose --env-file deploy/compose.validate.env -f compose.yaml -f deploy/compose.dedicated.yaml config --quiet
	test "$$(docker compose --env-file deploy/compose.validate.env -f compose.yaml config --services | sort | xargs)" = "backend broker frontend retrieval"
	test "$$(docker compose --env-file deploy/compose.validate.env -f deploy/compose.truenas-standalone.yaml config --services | sort | xargs)" = "backend broker code-intel frontend retrieval runner"
	test "$$(docker compose --env-file deploy/compose.validate.env -f compose.yaml -f deploy/compose.standalone.yaml config --services | sort | xargs)" = "backend broker code-intel frontend retrieval runner"
	test "$$(docker compose --env-file deploy/compose.validate.env -f compose.yaml -f deploy/compose.dedicated.yaml config --services | sort | xargs)" = "code-intel ingester node"
	docker compose --env-file deploy/compose.validate.env -f compose.yaml config --format json | jq -e '.services.backend.image == "ghcr.io/chris-arsenault/sulion/backend:test" and .services.backend.environment.SULION_DEPLOYMENT_ROLE == "control-plane" and .services.backend.environment.SULION_NODE_TRANSPORT == "remote" and .services.backend.environment.SULION_DOCKER_MODE == "none" and .services.backend.environment.SULION_SECRET_BROKER_REGISTRATION_TOKEN == null and (.services.backend.volumes // []) == [] and (.services.backend.ports // []) == [] and .services["code-intel"] == null and .services.runner == null and .services.frontend.environment.SULION_RETRIEVAL_UPSTREAM == "retrieval:8083"'
	docker compose --env-file deploy/compose.validate.env -f deploy/compose.truenas-standalone.yaml config --format json | jq -e '.services.backend.environment.SULION_DEPLOYMENT_ROLE == "standalone" and .services.backend.environment.SULION_NODE_TRANSPORT == "loopback" and .services.backend.environment.SULION_DOCKER_MODE == "brokered" and .services.backend.environment.SULION_SECRET_BROKER_REGISTRATION_TOKEN == "test" and any(.services.backend.volumes[]; .target == "/home/dev") and .services["code-intel"].profiles == null and .services.runner.profiles == null'
	docker compose --env-file deploy/compose.validate.env -f compose.yaml -f deploy/compose.dedicated.yaml config --format json | jq -e '.services.backend == null and .services.broker == null and .services.retrieval == null and .services.frontend == null and .services.runner == null and .services.node.user == "root" and .services.node.network_mode == "host" and .services.node.environment.SULION_NODE_CONTROL_URL == "wss://sulion.services.ahara.io/ws/nodes" and .services.node.environment.SULION_DOCKER_MODE == "direct" and .services.node.environment.SULION_SECRET_BROKER_URL == "https://sulion.services.ahara.io/broker" and .services.node.environment.SULION_RETRIEVAL_URL == "https://sulion.services.ahara.io/retrieval" and .services.node.environment.SULION_SECRET_BROKER_REGISTRATION_TOKEN == "test" and .services.node.environment.DOCKER_HOST == "unix:///var/run/docker.sock" and .services.node.environment.HOME == "/home/sulion" and .services.node.environment.SULION_REPOS_ROOT == "/home/sulion/repos" and any(.services.node.volumes[]; .source == "/home/sulion" and .target == "/home/sulion") and any(.services.node.volumes[]; .source == "/var/run/docker.sock" and .target == "/var/run/docker.sock") and .services.ingester.network_mode == "host" and .services.ingester.environment.HOME == "/home/sulion" and all(.services.ingester.volumes[]; .target != "/home/sulion/repos" and .target != "/home/sulion/workspaces") and .services["code-intel"].environment.SULION_CODE_INTEL_ALLOWED_ROOTS == "/home/sulion/repos,/home/sulion/workspaces" and any(.services["code-intel"].volumes[]; .source == "/home/sulion/repos" and .target == "/home/sulion/repos") and any(.services["code-intel"].volumes[]; .source == "/home/sulion/workspaces" and .target == "/home/sulion/workspaces") and .networks.sulion.driver_opts."com.docker.network.bridge.name" == "sulion0"'

validate-nix:
	docker build --target evaluate -f nix/Dockerfile.check -t sulion-nix-evaluate:test .

test-nix:
	docker build --target test -f nix/Dockerfile.check -t sulion-nix-vm:test .

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

screenshots:
	cd frontend && SULION_SCREENSHOT_TOUR=1 pnpm exec playwright test 99-tour.spec.ts
	python3 scripts/crop_screenshots.py
