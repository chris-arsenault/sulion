#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(SCRIPT_DIR, "..");
const BACKEND_PORT = Number(process.env.SULION_E2E_BACKEND_PORT ?? "38080");
const FRONTEND_PORT = Number(process.env.SULION_E2E_FRONTEND_PORT ?? "34173");
const BACKEND_BASE_URL = `http://127.0.0.1:${BACKEND_PORT}`;
const FRONTEND_URL = `http://127.0.0.1:${FRONTEND_PORT}`;
const BACKEND_IMAGE = process.env.SULION_E2E_BACKEND_IMAGE ?? "sulion-e2e-backend:local";
const BACKEND_RUNTIME_IMAGE =
  process.env.SULION_E2E_BACKEND_RUNTIME_IMAGE ?? `${BACKEND_IMAGE}-runtime`;
const BROKER_PORT = Number(process.env.SULION_E2E_BROKER_PORT ?? "38081");
const BROKER_BASE_URL = `http://127.0.0.1:${BROKER_PORT}`;
const BROKER_IMAGE = process.env.SULION_E2E_BROKER_IMAGE ?? "sulion-e2e-broker:local";
const E2E_AUTH_CLIENT_ID = "sulion-e2e-client";
const E2E_BROKER_REGISTRATION_TOKEN = "sulion-e2e-registration-token";

// Bind-mount sources must be visible to the docker daemon. In a managed PTY
// this script runs inside a container while the daemon is the host's: /tmp is
// container-local and the daemon would mount an empty directory in its place,
// but $HOME is bind-mounted at the same absolute path on both sides. Keep
// everything the containers mount under it.
const tmpParent =
  process.env.SULION_E2E_TMP_ROOT ?? path.join(os.homedir(), ".cache", "sulion-e2e");
fs.mkdirSync(tmpParent, { recursive: true });
const tmpRoot = fs.mkdtempSync(path.join(tmpParent, "stack-"));
const containerPaths = {
  reposRoot: "/home/sulion/repos",
  workspacesRoot: "/home/sulion/workspaces",
  libraryRoot: "/home/sulion/.sulion/library",
  claudeProjects: "/home/sulion/.claude/projects",
  codexSessions: "/home/sulion/.codex/sessions",
};

let dockerNetworkName = "";
let dbContainerName = "";
let backendContainerName = "";
let nodeContainerName = "";
let ingesterContainerName = "";
let brokerContainerName = "";
let authContainerName = "";
let nodeVolumeName = "";
let nodeKeyVolumeName = "";
let frontendProcess = null;
let shuttingDown = false;
let e2eAccessToken = "";

async function main() {
  try {
    cleanupStaleResources();
    prepareBackendDist();
    buildBackendImage();
    buildBrokerImage();

    const dbUrl = await ensureDb();
    const authIssuerUrl = startAuthFixture();
    startBrokerContainer(dbUrl, authIssuerUrl);
    await waitForHttp(`${BROKER_BASE_URL}/health`, 120_000);
    startBackendContainer(dbUrl);
    await waitForHttp(`${BACKEND_BASE_URL}/health`, 180_000);
    const nodeId = prepareNodeVolumes();
    startNodeContainer(dbUrl, nodeId);
    startIngesterContainer(dbUrl);
    await approveNodePairing(nodeId, 90_000);
    // Play the host's activation role: the control plane delivers runtime
    // config after approval, and the node waits until its own environment
    // carries the delivered digest. On the enclave a root timer recreates
    // the container; here the script does.
    const digest = await waitForDeliveredDigest(90_000);
    runCommand("docker-rm-node", "docker", ["rm", "-f", nodeContainerName], {
      cwd: REPO_ROOT,
    });
    startNodeContainer(dbUrl, nodeId, digest);
    await waitForNode(nodeId, 60_000);

    runCommand(
      "seed",
      "docker",
      [
        "exec",
        // As the development user: the node process runs as 7321 and git
        // refuses root-owned repositories (safe.directory), which silently
        // strips every git summary the sidebar renders.
        "--user",
        "7321:7321",
        "-e",
        `SULION_E2E_DB_URL=${dbUrl}`,
        "-e",
        `SULION_E2E_BASE_URL=http://${backendContainerName}:8080`,
        "-e",
        `SULION_REPOS_ROOT=${containerPaths.reposRoot}`,
        "-e",
        `SULION_LIBRARY_ROOT=${containerPaths.libraryRoot}`,
        "-e",
        `SULION_CLAUDE_PROJECTS=${containerPaths.claudeProjects}`,
        "-e",
        `SULION_CODEX_SESSIONS=${containerPaths.codexSessions}`,
        nodeContainerName,
        "/usr/local/bin/e2e_seed",
      ],
      { cwd: REPO_ROOT },
    );

    runCommand("restart-control", "docker", ["restart", backendContainerName], {
      cwd: REPO_ROOT,
    });
    await waitForHttp(`${BACKEND_BASE_URL}/health`, 60_000);
    await waitForNode(nodeId, 60_000);
    // Repo discovery is eventual: the node rescans its repos root on a 30 s
    // interval, so "stack ready" must mean the seeded repos are actually
    // served — the first specs assert on the sidebar within seconds.
    await waitForSeededRepos(["atlas", "zephyr"], 90_000);

    frontendProcess = startProcess(
      "frontend",
      "pnpm",
      ["--dir", "frontend", "dev", "--host", "127.0.0.1", "--port", String(FRONTEND_PORT)],
      {
        cwd: REPO_ROOT,
        env: {
          ...process.env,
          SULION_API_TARGET: BACKEND_BASE_URL,
          SULION_BROKER_TARGET: BROKER_BASE_URL,
          SULION_WS_TARGET: BACKEND_BASE_URL.replace("http://", "ws://"),
          VITE_SULION_E2E: "1",
          VITE_SULION_E2E_ACCESS_TOKEN: e2eAccessToken,
        },
      },
    );

    await waitForHttp(FRONTEND_URL, 120_000);
    console.log(`sulion e2e stack ready: ${FRONTEND_URL}`);
  } catch (error) {
    console.error(error instanceof Error ? error.stack : String(error));
    // The containers are removed on cleanup, so this is the only chance to
    // see why one of them died. Without it a startup failure reports only
    // "timed out waiting for <url>".
    dumpContainerLogs();
    await cleanup();
    process.exit(1);
  }
}

function dumpContainerLogs() {
  const containers = [
    ["db", dbContainerName],
    ["auth", authContainerName],
    ["broker", brokerContainerName],
    ["backend", backendContainerName],
    ["node", nodeContainerName],
    ["ingester", ingesterContainerName],
  ];
  for (const [label, name] of containers) {
    if (!name) continue;
    const result = spawnSync("docker", ["logs", "--tail", "80", name], {
      encoding: "utf8",
    });
    const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
    console.error(
      `--- ${label} (${name}) logs ---\n${output || "(container gone or produced no output)"}`,
    );
  }
}

function cleanupStaleResources() {
  const staleContainers = spawnSync(
    "bash",
    [
      "-lc",
      "docker ps -a --format '{{.Names}}' | rg '^sulion-e2e-(auth|backend|broker|db|ingester|node)-' || true",
    ],
    { cwd: REPO_ROOT, encoding: "utf8" },
  );
  if (staleContainers.status === 0) {
    for (const name of staleContainers.stdout.split("\n").map((value) => value.trim()).filter(Boolean)) {
      spawnSync("docker", ["rm", "-f", name], { cwd: REPO_ROOT, stdio: "ignore" });
    }
  }

  const staleNetworks = spawnSync(
    "bash",
    [
      "-lc",
      "docker network ls --format '{{.Name}}' | rg '^sulion-e2e-net-' || true",
    ],
    { cwd: REPO_ROOT, encoding: "utf8" },
  );
  if (staleNetworks.status === 0) {
    for (const name of staleNetworks.stdout.split("\n").map((value) => value.trim()).filter(Boolean)) {
      spawnSync("docker", ["network", "rm", name], { cwd: REPO_ROOT, stdio: "ignore" });
    }
  }
}

function prepareBackendDist() {
  runCommand(
    "cargo-build",
    "cargo",
    [
      "build",
      "--manifest-path",
      "backend/Cargo.toml",
      "--bin",
      "sulion",
      "--bin",
      "sulion-node",
      "--bin",
      "sulion-devenv",
      "--bin",
      "sulion-ingester",
      "--bin",
      "e2e_seed",
      "--bin",
      "sulion-broker",
    ],
    { cwd: REPO_ROOT },
  );

  const distDir = path.join(REPO_ROOT, "backend", "dist");
  fs.mkdirSync(distDir, { recursive: true });
  copyExecutable(
    path.join(REPO_ROOT, "backend", "target", "debug", "sulion"),
    path.join(distDir, "sulion"),
  );
  copyExecutable(
    path.join(REPO_ROOT, "backend", "target", "debug", "sulion-node"),
    path.join(distDir, "sulion-node"),
  );
  copyExecutable(
    path.join(REPO_ROOT, "backend", "target", "debug", "sulion-devenv"),
    path.join(distDir, "sulion-devenv"),
  );
  copyExecutable(
    path.join(REPO_ROOT, "backend", "target", "debug", "sulion-ingester"),
    path.join(distDir, "sulion-ingester"),
  );
  copyExecutable(
    path.join(REPO_ROOT, "backend", "target", "debug", "e2e_seed"),
    path.join(distDir, "e2e_seed"),
  );

  const brokerDistDir = path.join(REPO_ROOT, "broker", "dist");
  fs.mkdirSync(brokerDistDir, { recursive: true });
  copyExecutable(
    path.join(REPO_ROOT, "backend", "target", "debug", "sulion-broker"),
    path.join(brokerDistDir, "sulion-broker"),
  );
}

function copyExecutable(source, target) {
  fs.copyFileSync(source, target);
  fs.chmodSync(target, 0o755);
}

function buildBackendImage() {
  runCommand(
    "docker-build-runtime",
    "docker",
    ["build", "-t", BACKEND_RUNTIME_IMAGE, "backend"],
    { cwd: REPO_ROOT },
  );
  runCommand(
    "docker-build-e2e",
    "docker",
    [
      "build",
      "-f",
      "backend/Dockerfile.e2e",
      "--build-arg",
      `BASE_IMAGE=${BACKEND_RUNTIME_IMAGE}`,
      "-t",
      BACKEND_IMAGE,
      "backend",
    ],
    { cwd: REPO_ROOT },
  );
}

function buildBrokerImage() {
  runCommand(
    "docker-build-broker",
    "docker",
    ["build", "-t", BROKER_IMAGE, "broker"],
    { cwd: REPO_ROOT },
  );
}

function startAuthFixture() {
  authContainerName = `sulion-e2e-auth-${process.pid}`;
  const issuerUrl = `http://${authContainerName}:8099`;
  const { privateKey, publicKey } = crypto.generateKeyPairSync("rsa", {
    modulusLength: 2048,
  });
  const kid = "sulion-e2e";
  const jwk = publicKey.export({ format: "jwk" });
  const jwks = {
    keys: [
      {
        ...jwk,
        kid,
        use: "sig",
        alg: "RS256",
      },
    ],
  };

  const authRoot = path.join(tmpRoot, "auth");
  const wellKnown = path.join(authRoot, ".well-known");
  fs.mkdirSync(wellKnown, { recursive: true });
  fs.writeFileSync(path.join(wellKnown, "jwks.json"), JSON.stringify(jwks));

  e2eAccessToken = signJwt(
    {
      iss: issuerUrl,
      sub: "sulion-e2e-user",
      client_id: E2E_AUTH_CLIENT_ID,
      token_use: "access",
      username: "sulion-e2e",
      exp: Math.floor(Date.now() / 1000) + 60 * 60,
      iat: Math.floor(Date.now() / 1000),
    },
    privateKey,
    kid,
  );

  runCommand(
    "docker-run-auth",
    "docker",
    [
      "run",
      // No --rm: a container that dies must stay inspectable for the
      // failure-path log dump; cleanup force-removes everything anyway.
      "-d",
      "--name",
      authContainerName,
      "--network",
      dockerNetworkName,
      "-v",
      `${authRoot}:/srv/auth:ro`,
      "--entrypoint",
      "python3",
      BACKEND_RUNTIME_IMAGE,
      "-m",
      "http.server",
      "8099",
      "--bind",
      "0.0.0.0",
      "--directory",
      "/srv/auth",
    ],
    { cwd: REPO_ROOT },
  );
  return issuerUrl;
}

function signJwt(payload, privateKey, kid) {
  const header = { alg: "RS256", typ: "JWT", kid };
  const encodedHeader = base64url(JSON.stringify(header));
  const encodedPayload = base64url(JSON.stringify(payload));
  const signingInput = `${encodedHeader}.${encodedPayload}`;
  const signature = crypto.sign("RSA-SHA256", Buffer.from(signingInput), privateKey);
  return `${signingInput}.${base64url(signature)}`;
}

function base64url(value) {
  return Buffer.from(value)
    .toString("base64")
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/u, "");
}

function randomPrintableKey(length) {
  const bytes = Buffer.alloc(length);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = crypto.randomInt(33, 127);
  }
  return bytes;
}

function startBrokerContainer(dbUrl, authIssuerUrl) {
  brokerContainerName = `sulion-e2e-broker-${process.pid}`;
  const brokerStateDir = path.join(tmpRoot, "broker");
  fs.mkdirSync(brokerStateDir, { recursive: true });
  fs.writeFileSync(path.join(brokerStateDir, "master.key"), randomPrintableKey(32));

  runCommand(
    "docker-run-broker",
    "docker",
    [
      "run",
      // No --rm: a container that dies must stay inspectable for the
      // failure-path log dump; cleanup force-removes everything anyway.
      "-d",
      "--name",
      brokerContainerName,
      "--network",
      dockerNetworkName,
      "-p",
      `127.0.0.1:${BROKER_PORT}:8081`,
      "-e",
      "SULION_SECRET_BROKER_LISTEN=0.0.0.0:8081",
      "-e",
      // Its own database, as in production: broker and backend each own a
      // sqlx migration history, and sharing one database makes the second
      // migrator refuse the first one's checksums.
      `SULION_SECRET_BROKER_DB_URL=${dbUrl.replace(/\/sulion$/u, "/sulion_broker")}`,
      "-e",
      "SULION_SECRET_BROKER_MASTER_KEY_PATH=/var/lib/sulion-broker/master.key",
      "-e",
      `SULION_AUTH_ISSUER_URL=${authIssuerUrl}`,
      "-e",
      `SULION_AUTH_CLIENT_ID=${E2E_AUTH_CLIENT_ID}`,
      "-e",
      `SULION_SECRET_BROKER_REGISTRATION_TOKEN=${E2E_BROKER_REGISTRATION_TOKEN}`,
      "-v",
      `${brokerStateDir}:/var/lib/sulion-broker:ro`,
      BROKER_IMAGE,
    ],
    { cwd: REPO_ROOT },
  );
}

function startBackendContainer(dbUrl) {
  backendContainerName = `sulion-e2e-backend-${process.pid}`;
  const args = [
    "run",
    // No --rm: see the service-container note above.
    "-d",
    "--name",
    backendContainerName,
    "-p",
    `127.0.0.1:${BACKEND_PORT}:8080`,
    "-e",
    `SULION_DB_URL=${dbUrl}`,
    "-e",
    "SULION_LISTEN=0.0.0.0:8080",
    "-e",
    "SULION_DEPLOYMENT_ROLE=control-plane",
    "-e",
    "SULION_NODE_TRANSPORT=remote",
    "-e",
    "SULION_REPOS_ROOT=/var/empty/sulion/repos",
    "-e",
    "SULION_WORKSPACES_ROOT=/var/empty/sulion/workspaces",
    "-e",
    "SULION_CLAUDE_PROJECTS=/var/empty/sulion/claude-projects",
    "-e",
    "SULION_CODEX_SESSIONS=/var/empty/sulion/codex-sessions",
    "-e",
    // Writable: the library API stores prompt/reference markdown on the
    // control plane's local disk; the /var/empty sentinel turns every save
    // into a 500.
    "SULION_LIBRARY_ROOT=/home/sulion/library",
    "-e",
    "SULION_ENABLE_E2E_FIXTURES=1",
    // The suite's node lives on an ephemeral Docker network, not the dedicated
    // LAN, so the node source boundary is explicitly disabled here rather than
    // inheriting the production default.
    "-e",
    "SULION_NODE_LAN_CIDR=",
    "-e",
    `SULION_SECRET_BROKER_URL=http://${brokerContainerName}:8081`,
    "-e",
    `SULION_SECRET_BROKER_REGISTRATION_TOKEN=${E2E_BROKER_REGISTRATION_TOKEN}`,
  ];
  if (dockerNetworkName) {
    args.push("--network", dockerNetworkName);
  }
  args.push(BACKEND_IMAGE);

  runCommand("docker-run-backend", "docker", args, { cwd: REPO_ROOT });
}

// The only node identity pairing accepts. The node generates its own key on
// first start, requests pairing over /ws/nodes, and waits; approval is the
// whole enrollment (there are no enrollment tokens).
const DEDICATED_NODE_ID = "019d4f28-88ac-7a80-932c-b0f53a0708f4";

function prepareNodeVolumes() {
  nodeVolumeName = `sulion-e2e-node-home-${process.pid}`;
  nodeKeyVolumeName = `sulion-e2e-node-key-${process.pid}`;
  runCommand("docker-volume-node-home", "docker", ["volume", "create", nodeVolumeName], {
    cwd: REPO_ROOT,
  });
  runCommand("docker-volume-node-key", "docker", ["volume", "create", nodeKeyVolumeName], {
    cwd: REPO_ROOT,
  });
  runCommand(
    "initialize-node-home",
    "docker",
    [
      "run",
      "--rm",
      "--user",
      "root",
      "-v",
      `${nodeVolumeName}:/home/sulion`,
      "--entrypoint",
      "chown",
      BACKEND_IMAGE,
      "-R",
      "7321:7321",
      "/home/sulion",
    ],
    { cwd: REPO_ROOT },
  );
  return DEDICATED_NODE_ID;
}

/// Reads the delivered-config digest from the node's own log line. The node
/// writes it right after approval; until then the line does not exist.
async function waitForDeliveredDigest(timeoutMs) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const logs = spawnSync("docker", ["logs", nodeContainerName], { encoding: "utf8" });
    const match = /"message":"wrote delivered node runtime configuration"[^\n]*?"digest":"([A-Za-z0-9_-]+)"/u.exec(
      `${logs.stdout ?? ""}${logs.stderr ?? ""}`,
    );
    if (match) {
      return match[1];
    }
    await sleep(500);
  }
  throw new Error("node never wrote its delivered runtime configuration");
}

/// The node dials in, records a pending pairing, and retries until an
/// operator approves. This plays the operator: approval 404s until the
/// pairing request has landed, so poll.
async function approveNodePairing(nodeId, timeoutMs) {
  const start = Date.now();
  let lastFailure = "";
  while (Date.now() - start < timeoutMs) {
    try {
      const response = await fetch(`${BACKEND_BASE_URL}/api/nodes/${nodeId}/approve`, {
        method: "POST",
        headers: { authorization: `Bearer ${e2eAccessToken}` },
      });
      if (response.ok) {
        return;
      }
      lastFailure = `HTTP ${response.status}: ${await response.text()}`;
    } catch (error) {
      lastFailure = String(error);
    }
    await sleep(500);
  }
  throw new Error(`node pairing approval never succeeded (${lastFailure})`);
}

function startNodeContainer(dbUrl, nodeId, configDigest = "") {
  nodeContainerName = `sulion-e2e-node-${process.pid}`;
  runCommand(
    "docker-run-node",
    "docker",
    [
      "run",
      // No --rm: a container that dies must stay inspectable for the
      // failure-path log dump; cleanup force-removes everything anyway.
      "-d",
      "--name",
      nodeContainerName,
      "--user",
      "root",
      "--network",
      dockerNetworkName,
      "-e",
      `SULION_DB_URL=${dbUrl}`,
      "-e",
      `SULION_NODE_ID=${nodeId}`,
      "-e",
      `SULION_NODE_CONTROL_URL=ws://${backendContainerName}:8080/ws/nodes`,
      "-e",
      "SULION_NODE_PRIVATE_KEY_PATH=/var/lib/sulion-node/private-key.pk8",
      "-e",
      "SULION_NODE_RUN_USER=sulion",
      "-e",
      "SULION_NODE_RUN_UID=7321",
      "-e",
      "SULION_NODE_RUN_GID=7321",
      "-e",
      // Present on the activation restart, absent on the first start: the
      // node treats a matching digest as "the host already applied what the
      // control plane delivered".
      `SULION_NODE_CONFIG_DIGEST=${configDigest}`,
      "-e",
      // The container starts as root (docker would default HOME=/root) but
      // the entrypoint seeds the development home as the sulion user, same
      // as the compose graph's node service.
      "HOME=/home/sulion",
      "-e",
      "SULION_NODE_ALLOW_INSECURE_WS=1",
      "-e",
      "SULION_DOCKER_MODE=none",
      "-e",
      `SULION_REPOS_ROOT=${containerPaths.reposRoot}`,
      "-e",
      `SULION_WORKSPACES_ROOT=${containerPaths.workspacesRoot}`,
      "-e",
      `SULION_CLAUDE_PROJECTS=${containerPaths.claudeProjects}`,
      "-e",
      `SULION_CODEX_SESSIONS=${containerPaths.codexSessions}`,
      "-e",
      "SULION_ENABLE_E2E_FIXTURES=1",
      "-e",
      `SULION_SECRET_BROKER_URL=http://${brokerContainerName}:8081`,
      "-e",
      `SULION_SECRET_BROKER_REGISTRATION_TOKEN=${E2E_BROKER_REGISTRATION_TOKEN}`,
      "-v",
      `${nodeVolumeName}:/home/sulion`,
      "-v",
      // Writable: the node generates its identity key here on first start
      // and records delivered configuration beside it.
      `${nodeKeyVolumeName}:/var/lib/sulion-node`,
      "--entrypoint",
      "/usr/bin/dumb-init",
      BACKEND_IMAGE,
      "--",
      "/opt/sulion/node-entrypoint.sh",
    ],
    { cwd: REPO_ROOT },
  );
}

function startIngesterContainer(dbUrl) {
  ingesterContainerName = `sulion-e2e-ingester-${process.pid}`;
  runCommand(
    "docker-run-ingester",
    "docker",
    [
      "run",
      // No --rm: a container that dies must stay inspectable for the
      // failure-path log dump; cleanup force-removes everything anyway.
      "-d",
      "--name",
      ingesterContainerName,
      "--network",
      dockerNetworkName,
      "-e",
      `SULION_DB_URL=${dbUrl}`,
      "-e",
      `SULION_CLAUDE_PROJECTS=${containerPaths.claudeProjects}`,
      "-e",
      `SULION_CODEX_SESSIONS=${containerPaths.codexSessions}`,
      "-v",
      `${nodeVolumeName}:/home/sulion:ro`,
      "--entrypoint",
      "/usr/bin/dumb-init",
      BACKEND_IMAGE,
      "--",
      "/usr/local/bin/sulion-ingester",
    ],
    { cwd: REPO_ROOT },
  );
}

async function waitForSeededRepos(names, timeoutMs) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const response = await fetch(`${BACKEND_BASE_URL}/api/app-state`, {
        headers: { authorization: `Bearer ${e2eAccessToken}` },
      });
      if (response.ok) {
        const state = await response.json();
        const repos = new Set((state.repos ?? []).map((repo) => repo.name));
        if (names.every((name) => repos.has(name))) {
          return;
        }
      }
    } catch {
      // control or node still converging
    }
    await sleep(1000);
  }
  throw new Error(`seeded repos [${names.join(", ")}] never appeared in app-state`);
}

async function waitForNode(nodeId, timeoutMs) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const response = await fetch(`${BACKEND_BASE_URL}/api/app-state`, {
        headers: { authorization: `Bearer ${e2eAccessToken}` },
      });
      if (response.ok) {
        const state = await response.json();
        if (
          state.nodes?.some(
            (node) => node.id === nodeId && node.connection_state === "connected",
          )
        ) {
          return;
        }
      }
    } catch {
      // node or control is still converging
    }
    await sleep(250);
  }
  throw new Error(`timed out waiting for development node ${nodeId}`);
}

function startProcess(label, command, args, options) {
  const child = spawn(command, args, {
    ...options,
    stdio: "inherit",
  });
  child.on("exit", (code, signal) => {
    if (shuttingDown) return;
    console.error(
      `${label} exited unexpectedly (code=${code ?? "null"} signal=${signal ?? "null"})`,
    );
    void cleanup().finally(() => process.exit(code ?? 1));
  });
  return child;
}

function runCommand(label, command, args, options) {
  const result = spawnSync(command, args, {
    ...options,
    stdio: "inherit",
  });
  if (result.status === 0) {
    return;
  }
  throw new Error(`${label} failed with status ${result.status ?? "unknown"}`);
}

function runCommandCaptured(label, command, args, options) {
  const result = spawnSync(command, args, {
    ...options,
    encoding: "utf8",
  });
  if (result.status === 0) {
    return result.stdout;
  }
  throw new Error(
    `${label} failed with status ${result.status ?? "unknown"}: ${result.stderr}`,
  );
}

async function ensureDb() {
  dockerNetworkName = `sulion-e2e-net-${process.pid}`;
  runCommand("docker-network", "docker", ["network", "create", dockerNetworkName], {
    cwd: REPO_ROOT,
  });

  if (process.env.SULION_E2E_DB_URL) {
    return process.env.SULION_E2E_DB_URL;
  }

  dbContainerName = `sulion-e2e-db-${process.pid}`;

  runCommand(
    "docker-run-db",
    "docker",
    [
      "run",
      // No --rm: a container that dies must stay inspectable for the
      // failure-path log dump; cleanup force-removes everything anyway.
      "-d",
      "--name",
      dbContainerName,
      "--network",
      dockerNetworkName,
      "-e",
      "POSTGRES_PASSWORD=testpass",
      "-e",
      "POSTGRES_DB=sulion",
      "docker.io/library/postgres:16",
    ],
    { cwd: REPO_ROOT },
  );

  await waitForPostgres(dbContainerName, 30_000);
  runCommand(
    "create-broker-db",
    "docker",
    [
      "exec",
      dbContainerName,
      "psql",
      "-U",
      "postgres",
      "-c",
      "CREATE DATABASE sulion_broker",
    ],
    { cwd: REPO_ROOT },
  );
  return `postgres://postgres:testpass@${dbContainerName}:5432/sulion`;
}

async function waitForPostgres(containerName, timeoutMs) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const probe = spawnSync(
      "docker",
      ["exec", containerName, "pg_isready", "-U", "postgres", "-d", "sulion"],
      { stdio: "ignore" },
    );
    if (probe.status === 0) {
      return;
    }
    await sleep(500);
  }
  throw new Error("timed out waiting for postgres");
}

async function waitForHttp(url, timeoutMs) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
    } catch {
      // service not ready yet
    }
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${url}`);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function cleanup() {
  if (shuttingDown) return;
  shuttingDown = true;

  if (frontendProcess && !frontendProcess.killed) {
    frontendProcess.kill("SIGTERM");
    await sleep(500);
    if (frontendProcess.exitCode === null && frontendProcess.signalCode === null) {
      frontendProcess.kill("SIGKILL");
    }
  }

  if (backendContainerName) {
    spawnSync("docker", ["rm", "-f", backendContainerName], { stdio: "ignore" });
    backendContainerName = "";
  }
  if (nodeContainerName) {
    spawnSync("docker", ["rm", "-f", nodeContainerName], { stdio: "ignore" });
    nodeContainerName = "";
  }
  if (ingesterContainerName) {
    spawnSync("docker", ["rm", "-f", ingesterContainerName], { stdio: "ignore" });
    ingesterContainerName = "";
  }
  if (brokerContainerName) {
    spawnSync("docker", ["rm", "-f", brokerContainerName], { stdio: "ignore" });
    brokerContainerName = "";
  }
  if (authContainerName) {
    spawnSync("docker", ["rm", "-f", authContainerName], { stdio: "ignore" });
    authContainerName = "";
  }
  if (dbContainerName) {
    spawnSync("docker", ["rm", "-f", dbContainerName], { stdio: "ignore" });
    dbContainerName = "";
  }
  if (dockerNetworkName) {
    spawnSync("docker", ["network", "rm", dockerNetworkName], { stdio: "ignore" });
    dockerNetworkName = "";
  }
  if (nodeVolumeName) {
    spawnSync("docker", ["volume", "rm", "-f", nodeVolumeName], { stdio: "ignore" });
    nodeVolumeName = "";
  }
  if (nodeKeyVolumeName) {
    spawnSync("docker", ["volume", "rm", "-f", nodeKeyVolumeName], { stdio: "ignore" });
    nodeKeyVolumeName = "";
  }

  fs.rmSync(tmpRoot, { recursive: true, force: true });
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    void cleanup().finally(() => process.exit(0));
  });
}

process.on("exit", () => {
  if (!shuttingDown) {
    fs.rmSync(tmpRoot, { recursive: true, force: true });
  }
});

await main();
await new Promise(() => {});
