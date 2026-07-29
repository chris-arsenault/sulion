#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BACKEND_DIR="${REPO_ROOT}/backend"

TEST_TARGETS=(
  db_integration
  correlate_integration
  rest_integration
  retrieval_integration
  code_intel_integration
  device_integration
  workspace_integration
  pty_integration
  ws_integration
  node_protocol_integration
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

# Retries because this runs right after `docker run -d`: the port forwarder
# binds a moment after the daemon reports the container started. This only
# proves the address is routable, not that postgres is accepting queries --
# wait_for_postgres does that.
port_is_open() {
  local host="$1" port="$2" attempt
  for attempt in $(seq 1 10); do
    if timeout 2 bash -c "cat < /dev/null > /dev/tcp/${host}/${port}" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

start_postgres_container() {
  # Always publish. Whether the mapped port is reachable depends on the caller
  # sharing a network namespace with the docker daemon, which differs between
  # a plain host, a managed PTY, and a remote runner -- so the address is
  # probed below rather than inferred from which docker binary is on PATH.
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
    -p "127.0.0.1::${DOCKER_CONTAINER_PORT}"
  )

  docker_args+=(docker.io/library/postgres:16)
  docker "${docker_args[@]}" >/dev/null

  local mapped
  mapped="$(docker port "${DOCKER_CONTAINER_NAME}" "${DOCKER_CONTAINER_PORT}/tcp" 2>/dev/null | awk -F: 'END { print $NF }')"

  if [[ -n "${mapped}" ]] && port_is_open 127.0.0.1 "${mapped}"; then
    DOCKER_DB_HOST="127.0.0.1"
    DOCKER_DB_PORT="${mapped}"
    return 0
  fi

  # No usable published port: the daemon is elsewhere, so reach the container
  # by name on the shared docker network instead.
  if port_is_open "${DOCKER_CONTAINER_NAME}" "${DOCKER_CONTAINER_PORT}"; then
    DOCKER_DB_HOST="${DOCKER_CONTAINER_NAME}"
    DOCKER_DB_PORT="${DOCKER_CONTAINER_PORT}"
    return 0
  fi

  echo "sulion: postgres test container is not reachable at 127.0.0.1:${mapped:-<unmapped>}" >&2
  echo "sulion: nor by container name ${DOCKER_CONTAINER_NAME}:${DOCKER_CONTAINER_PORT}" >&2
  echo "sulion: set SULION_TEST_DB to a reachable database to run these tests" >&2
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

  start_postgres_container
  wait_for_postgres

  export SULION_TEST_DB="postgres://postgres:testpass@${DOCKER_DB_HOST}:${DOCKER_DB_PORT}/sulion"
}

run_targets() {
  local cargo_target_args=()
  local target
  for target in "${TEST_TARGETS[@]}"; do
    cargo_target_args+=(--test "${target}")
  done

  echo "==> cargo test --release --features ${INTEGRATION_FEATURE} ${cargo_target_args[*]} -- --test-threads=1"
  (
    cd "${BACKEND_DIR}"
    cargo test --release --features "${INTEGRATION_FEATURE}" "${cargo_target_args[@]}" -- --test-threads=1
  )
}

ensure_test_db
run_targets
