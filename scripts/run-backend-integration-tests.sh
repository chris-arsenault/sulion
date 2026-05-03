#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BACKEND_DIR="${REPO_ROOT}/backend"

TEST_TARGETS=(
  db_integration
  correlate_integration
  rest_integration
  workspace_integration
  pty_integration
  ws_integration
  ingester_integration
)
INTEGRATION_FEATURE="integration-tests"

DOCKER_CONTAINER_NAME=""

cleanup() {
  if [[ -n "${DOCKER_CONTAINER_NAME}" ]]; then
    docker rm -f "${DOCKER_CONTAINER_NAME}" >/dev/null 2>&1 || true
  fi
}

wait_for_postgres() {
  local attempt
  for attempt in $(seq 1 30); do
    if docker exec "${DOCKER_CONTAINER_NAME}" pg_isready -U postgres -d sulion >/dev/null 2>&1 \
      && docker exec "${DOCKER_CONTAINER_NAME}" psql -U postgres -d sulion -c 'select 1' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done

  echo "sulion: postgres test container did not become ready" >&2
  return 1
}

ensure_test_db() {
  if [[ -n "${SULION_TEST_DB:-}" ]]; then
    return 0
  fi

  if ! command -v docker >/dev/null 2>&1; then
    echo "sulion: set SULION_TEST_DB or install Docker to run backend integration tests" >&2
    return 1
  fi

  DOCKER_CONTAINER_NAME="sulion-test-db-${PPID}-$$"
  trap cleanup EXIT

  docker run \
    --rm \
    -d \
    --name "${DOCKER_CONTAINER_NAME}" \
    -p "127.0.0.1::5432" \
    -e POSTGRES_PASSWORD=testpass \
    -e POSTGRES_DB=sulion \
    docker.io/library/postgres:16 >/dev/null

  wait_for_postgres

  local mapped_port
  mapped_port="$(docker port "${DOCKER_CONTAINER_NAME}" 5432/tcp | awk -F: 'END { print $NF }')"
  if [[ -z "${mapped_port}" ]]; then
    echo "sulion: failed to discover mapped postgres port" >&2
    return 1
  fi

  export SULION_TEST_DB="postgres://postgres:testpass@127.0.0.1:${mapped_port}/sulion"
}

run_target() {
  local target="$1"
  echo "==> cargo test --release --features ${INTEGRATION_FEATURE} --test ${target} -- --test-threads=1"
  (
    cd "${BACKEND_DIR}"
    cargo test --release --features "${INTEGRATION_FEATURE}" --test "${target}" -- --test-threads=1
  )
}

ensure_test_db

for target in "${TEST_TARGETS[@]}"; do
  run_target "${target}"
done
