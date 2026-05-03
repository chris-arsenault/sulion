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

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), "sulion-e2e-"));
const containerPaths = {
  reposRoot: "/tmp/sulion-e2e/repos",
  libraryRoot: "/tmp/sulion-e2e/library",
  claudeProjects: "/tmp/sulion-e2e/claude-projects",
  codexSessions: "/tmp/sulion-e2e/codex-sessions",
};

let dockerNetworkName = "";
let dbContainerName = "";
let backendContainerName = "";
let brokerContainerName = "";
let authContainerName = "";
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

    runCommand(
      "seed",
      "docker",
      [
        "exec",
        "-e",
        `SULION_E2E_DB_URL=${dbUrl}`,
        "-e",
        "SULION_E2E_BASE_URL=http://127.0.0.1:8080",
        "-e",
        `SULION_REPOS_ROOT=${containerPaths.reposRoot}`,
        "-e",
        `SULION_LIBRARY_ROOT=${containerPaths.libraryRoot}`,
        "-e",
        `SULION_CLAUDE_PROJECTS=${containerPaths.claudeProjects}`,
        "-e",
        `SULION_CODEX_SESSIONS=${containerPaths.codexSessions}`,
        backendContainerName,
        "/usr/local/bin/e2e_seed",
      ],
      { cwd: REPO_ROOT },
    );

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
    await cleanup();
    process.exit(1);
  }
}

function cleanupStaleResources() {
  const staleContainers = spawnSync(
    "bash",
    [
      "-lc",
      "docker ps -a --format '{{.Names}}' | rg '^sulion-e2e-(auth|backend|broker|db)-' || true",
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
      "--rm",
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
      "--rm",
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
      `SULION_SECRET_BROKER_DB_URL=${dbUrl}`,
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
    "--rm",
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
    `SULION_REPOS_ROOT=${containerPaths.reposRoot}`,
    "-e",
    `SULION_LIBRARY_ROOT=${containerPaths.libraryRoot}`,
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
  ];
  if (dockerNetworkName) {
    args.push("--network", dockerNetworkName);
  }
  args.push(BACKEND_IMAGE);

  runCommand("docker-run-backend", "docker", args, { cwd: REPO_ROOT });
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
      "--rm",
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
