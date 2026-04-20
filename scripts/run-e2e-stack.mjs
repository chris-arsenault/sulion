#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
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
let frontendProcess = null;
let shuttingDown = false;

async function main() {
  try {
    cleanupStaleResources();
    prepareBackendDist();
    buildBackendImage();

    const dbUrl = await ensureDb();
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
          SULION_WS_TARGET: BACKEND_BASE_URL.replace("http://", "ws://"),
          VITE_SULION_E2E: "1",
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
      "docker ps -a --format '{{.Names}}' | rg '^sulion-e2e-(backend|db)-' || true",
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
  if (process.env.SULION_E2E_DB_URL) {
    return process.env.SULION_E2E_DB_URL;
  }

  dockerNetworkName = `sulion-e2e-net-${process.pid}`;
  dbContainerName = `sulion-e2e-db-${process.pid}`;

  runCommand("docker-network", "docker", ["network", "create", dockerNetworkName], {
    cwd: REPO_ROOT,
  });
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
      "postgres:16",
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
