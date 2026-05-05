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
DOCKER_CONTAINER_PORT="5432"
DOCKER_DB_HOST="127.0.0.1"
DOCKER_DB_PORT="${DOCKER_CONTAINER_PORT}"

cleanup() {
  if [[ -n "${DOCKER_CONTAINER_NAME}" ]]; then
    docker rm -f "${DOCKER_CONTAINER_NAME}" >/dev/null 2>&1 || true
  fi
}

wait_for_postgres() {
  local attempt
  for attempt in $(seq 1 30); do
    local status
    status="$(
      docker inspect \
        --format '{{ if .State.Health }}{{ .State.Health.Status }}{{ else }}{{ .State.Status }}{{ end }}' \
        "${DOCKER_CONTAINER_NAME}" 2>/dev/null || true
    )"
    if [[ "${status}" == "healthy" ]]; then
      return 0
    fi
    if [[ "${status}" == "exited" || "${status}" == "dead" ]]; then
      docker logs "${DOCKER_CONTAINER_NAME}" >&2 || true
      echo "sulion: postgres test container exited before becoming ready" >&2
      return 1
    fi
    sleep 1
  done

  docker logs "${DOCKER_CONTAINER_NAME}" >&2 || true
  echo "sulion: postgres test container did not become ready" >&2
  return 1
}

using_sulion_runner() {
  [[ "$(command -v docker)" == "/opt/sulion/bin/docker" || -n "${SULION_RUNNER_URL:-}" ]]
}

start_postgres_container() {
  local docker_args=(
    run
    --rm
    -d
    --name "${DOCKER_CONTAINER_NAME}"
    --health-cmd "pg_isready -U postgres -d sulion -p ${DOCKER_CONTAINER_PORT}"
    --health-interval 1s
    --health-timeout 5s
    --health-retries 30
    -e POSTGRES_PASSWORD=testpass
    -e POSTGRES_DB=sulion
  )

  if using_sulion_runner; then
    DOCKER_DB_HOST="${DOCKER_CONTAINER_NAME}"
    DOCKER_DB_PORT="${DOCKER_CONTAINER_PORT}"
  else
    docker_args+=(-p "127.0.0.1::${DOCKER_CONTAINER_PORT}")
    DOCKER_DB_HOST="127.0.0.1"
  fi

  docker_args+=(docker.io/library/postgres:16)
  docker "${docker_args[@]}" >/dev/null

  if ! using_sulion_runner; then
    DOCKER_DB_PORT="$(docker port "${DOCKER_CONTAINER_NAME}" "${DOCKER_CONTAINER_PORT}/tcp" | awk -F: 'END { print $NF }')"
    if [[ -z "${DOCKER_DB_PORT}" ]]; then
      echo "sulion: failed to discover mapped postgres port" >&2
      return 1
    fi
  fi
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

  start_postgres_container
  wait_for_postgres

  export SULION_TEST_DB="postgres://postgres:testpass@${DOCKER_DB_HOST}:${DOCKER_DB_PORT}/sulion"
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
