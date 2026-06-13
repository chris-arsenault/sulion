use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::State;
use axum::Json;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use super::{
    RunnerConfig, RunnerError, RunnerState, OWNER_LABEL, OWNER_VALUE, PTY_LABEL, SULION_NETWORK,
};

const POSTGRES_SERVICE_LABEL: &str = "sulion.service";
const POSTGRES_SERVICE_VALUE: &str = "postgres";
const POSTGRES_SCOPE_LABEL: &str = "sulion.postgres.scope";
const POSTGRES_KEY_LABEL: &str = "sulion.postgres.key";
const POSTGRES_REPO_LABEL: &str = "sulion.repo";
const POSTGRES_WORKSPACE_LABEL: &str = "sulion.workspace_id";
const POSTGRES_IMAGE: &str = "docker.io/library/postgres:16";
const POSTGRES_PORT: &str = "5432";
const POSTGRES_USER: &str = "postgres";
const POSTGRES_DB: &str = "sulion";
const HEALTH_ATTEMPTS: usize = 60;
const HEALTH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PostgresMode {
    Reuse,
    Restart,
    Temp,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PostgresServiceRequest {
    cwd: String,
    pty_id: Option<String>,
    workspace_id: Option<String>,
    repo: Option<String>,
    mode: PostgresMode,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PostgresCleanupRequest {
    cwd: String,
    pty_id: Option<String>,
    workspace_id: Option<String>,
    repo: Option<String>,
    container_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PostgresServiceResponse {
    container_name: String,
    database_url: String,
    host: String,
    port: u16,
    user: String,
    password: String,
    database: String,
    reused: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PostgresCleanResponse {
    removed: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PostgresCleanupResponse {
    removed: bool,
}

struct PostgresContext {
    key: String,
    pty_id: Option<String>,
    workspace_id: Option<String>,
    repo: Option<String>,
    container_name: String,
    scope: &'static str,
}

#[derive(Debug, Deserialize)]
struct DockerInspect {
    #[serde(rename = "State")]
    state: DockerState,
    #[serde(rename = "Config")]
    config: DockerConfig,
}

#[derive(Debug, Deserialize)]
struct DockerState {
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "Health")]
    health: Option<DockerHealth>,
}

#[derive(Debug, Deserialize)]
struct DockerHealth {
    #[serde(rename = "Status")]
    status: String,
}

#[derive(Debug, Deserialize)]
struct DockerConfig {
    #[serde(rename = "Labels", default)]
    labels: HashMap<String, String>,
    #[serde(rename = "Env", default)]
    env: Vec<String>,
}

pub(super) async fn ensure_postgres(
    State(state): State<Arc<RunnerState>>,
    Json(request): Json<PostgresServiceRequest>,
) -> Result<Json<PostgresServiceResponse>, RunnerError> {
    state.ensure_postgres_service(request).await.map(Json)
}

pub(super) async fn cleanup_postgres(
    State(state): State<Arc<RunnerState>>,
    Json(request): Json<PostgresCleanupRequest>,
) -> Result<Json<PostgresCleanupResponse>, RunnerError> {
    state.cleanup_postgres_service(request).await.map(Json)
}

pub(super) async fn clean_postgres(
    State(state): State<Arc<RunnerState>>,
    Json(request): Json<PostgresServiceRequest>,
) -> Result<Json<PostgresCleanResponse>, RunnerError> {
    state.clean_postgres_service(request).await.map(Json)
}

impl RunnerState {
    async fn ensure_postgres_service(
        &self,
        request: PostgresServiceRequest,
    ) -> Result<PostgresServiceResponse, RunnerError> {
        let ctx = self.postgres_context(&request, request.mode)?;
        match request.mode {
            PostgresMode::Temp => self.start_postgres_and_wait(&ctx).await,
            PostgresMode::Restart => {
                self.remove_matching_postgres_container(&ctx.container_name, &ctx.key, true)
                    .await?;
                self.start_postgres_and_wait(&ctx).await
            }
            PostgresMode::Reuse => self.ensure_reusable_postgres(&ctx).await,
        }
    }

    async fn ensure_reusable_postgres(
        &self,
        ctx: &PostgresContext,
    ) -> Result<PostgresServiceResponse, RunnerError> {
        match inspect_container(&self.config.docker_bin, &ctx.container_name).await? {
            Some(info) => {
                validate_postgres_container(&ctx.container_name, &info, &ctx.key)?;
                if info.state.status == "running" {
                    let info = self.wait_for_postgres(&ctx.container_name).await?;
                    return postgres_response(&ctx.container_name, &info, true);
                }
                self.remove_matching_postgres_container(&ctx.container_name, &ctx.key, true)
                    .await?;
                self.start_postgres_and_wait(ctx).await
            }
            None => self.start_postgres_and_wait(ctx).await,
        }
    }

    async fn start_postgres_and_wait(
        &self,
        ctx: &PostgresContext,
    ) -> Result<PostgresServiceResponse, RunnerError> {
        match start_postgres_container(&self.config, ctx).await {
            Ok(()) => {
                let info = self.wait_for_postgres(&ctx.container_name).await?;
                postgres_response(&ctx.container_name, &info, false)
            }
            Err(err) if ctx.scope == "workspace" => {
                if let Some(info) =
                    inspect_container(&self.config.docker_bin, &ctx.container_name).await?
                {
                    validate_postgres_container(&ctx.container_name, &info, &ctx.key)?;
                    let info = self.wait_for_postgres(&ctx.container_name).await?;
                    return postgres_response(&ctx.container_name, &info, true);
                }
                Err(err)
            }
            Err(err) => Err(err),
        }
    }

    async fn wait_for_postgres(&self, container_name: &str) -> Result<DockerInspect, RunnerError> {
        for _ in 0..HEALTH_ATTEMPTS {
            if let Some(info) = inspect_container(&self.config.docker_bin, container_name).await? {
                let health = info
                    .state
                    .health
                    .as_ref()
                    .map(|health| health.status.as_str());
                if health == Some("healthy") || (health.is_none() && info.state.status == "running")
                {
                    return Ok(info);
                }
                if matches!(info.state.status.as_str(), "exited" | "dead") {
                    let logs = docker_logs(&self.config.docker_bin, container_name).await;
                    return Err(RunnerError::internal(format!(
                        "postgres container exited before becoming ready: {container_name}\n{}",
                        logs.unwrap_or_default()
                    )));
                }
            }
            tokio::time::sleep(HEALTH_INTERVAL).await;
        }

        let logs = docker_logs(&self.config.docker_bin, container_name).await;
        Err(RunnerError::internal(format!(
            "postgres container did not become ready: {container_name}\n{}",
            logs.unwrap_or_default()
        )))
    }

    async fn cleanup_postgres_service(
        &self,
        request: PostgresCleanupRequest,
    ) -> Result<PostgresCleanupResponse, RunnerError> {
        let ctx = self.postgres_context(
            &PostgresServiceRequest {
                cwd: request.cwd,
                pty_id: request.pty_id,
                workspace_id: request.workspace_id,
                repo: request.repo,
                mode: PostgresMode::Temp,
            },
            PostgresMode::Temp,
        )?;
        let removed = self
            .remove_matching_postgres_container(&request.container_name, &ctx.key, false)
            .await?;
        Ok(PostgresCleanupResponse { removed })
    }

    async fn clean_postgres_service(
        &self,
        request: PostgresServiceRequest,
    ) -> Result<PostgresCleanResponse, RunnerError> {
        let ctx = self.postgres_context(&request, PostgresMode::Reuse)?;
        let names = postgres_container_names_for_key(&self.config.docker_bin, &ctx.key).await?;
        let mut removed = Vec::new();
        for name in names {
            let Some(info) = inspect_container(&self.config.docker_bin, &name).await? else {
                continue;
            };
            validate_postgres_container(&name, &info, &ctx.key)?;
            let scope = info
                .config
                .labels
                .get(POSTGRES_SCOPE_LABEL)
                .map(String::as_str);
            let should_remove = scope == Some("temp") || info.state.status != "running";
            if should_remove
                && self
                    .remove_matching_postgres_container(&name, &ctx.key, false)
                    .await?
            {
                removed.push(name);
            }
        }
        Ok(PostgresCleanResponse { removed })
    }

    async fn remove_matching_postgres_container(
        &self,
        container_name: &str,
        key: &str,
        allow_workspace: bool,
    ) -> Result<bool, RunnerError> {
        let Some(info) = inspect_container(&self.config.docker_bin, container_name).await? else {
            return Ok(false);
        };
        validate_postgres_container(container_name, &info, key)?;
        let scope = info
            .config
            .labels
            .get(POSTGRES_SCOPE_LABEL)
            .map(String::as_str);
        if scope == Some("workspace") && !allow_workspace {
            return Err(RunnerError::forbidden(format!(
                "refusing to remove workspace postgres from cleanup path: {container_name}"
            )));
        }
        remove_container(&self.config.docker_bin, container_name).await?;
        Ok(true)
    }

    fn postgres_context(
        &self,
        request: &PostgresServiceRequest,
        mode: PostgresMode,
    ) -> Result<PostgresContext, RunnerError> {
        let cwd = self.validate_cwd(&request.cwd)?;
        let key = postgres_key(request.workspace_id.as_deref(), &cwd);
        let scope = if mode == PostgresMode::Temp {
            "temp"
        } else {
            "workspace"
        };
        let container_name = if mode == PostgresMode::Temp {
            format!("sulion-pg-temp-{}", Uuid::new_v4().simple())
        } else {
            format!("sulion-pg-{key}")
        };
        Ok(PostgresContext {
            key,
            pty_id: request.pty_id.clone(),
            workspace_id: request.workspace_id.clone(),
            repo: request.repo.clone(),
            container_name,
            scope,
        })
    }
}

async fn start_postgres_container(
    config: &RunnerConfig,
    ctx: &PostgresContext,
) -> Result<(), RunnerError> {
    let password = Uuid::new_v4().simple().to_string();
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        ctx.container_name.clone(),
        "--label".to_string(),
        format!("{OWNER_LABEL}={OWNER_VALUE}"),
        "--label".to_string(),
        format!("{PTY_LABEL}={}", ctx.pty_id.as_deref().unwrap_or("unknown")),
        "--label".to_string(),
        format!("{POSTGRES_SERVICE_LABEL}={POSTGRES_SERVICE_VALUE}"),
        "--label".to_string(),
        format!("{POSTGRES_SCOPE_LABEL}={}", ctx.scope),
        "--label".to_string(),
        format!("{POSTGRES_KEY_LABEL}={}", ctx.key),
        "--network".to_string(),
        SULION_NETWORK.to_string(),
        "--health-cmd".to_string(),
        format!("pg_isready -U {POSTGRES_USER} -d {POSTGRES_DB} -p {POSTGRES_PORT}"),
        "--health-interval".to_string(),
        "1s".to_string(),
        "--health-timeout".to_string(),
        "5s".to_string(),
        "--health-retries".to_string(),
        "30".to_string(),
        "-e".to_string(),
        format!("POSTGRES_PASSWORD={password}"),
        "-e".to_string(),
        format!("POSTGRES_DB={POSTGRES_DB}"),
    ];
    if let Some(workspace_id) = ctx.workspace_id.as_deref() {
        args.push("--label".to_string());
        args.push(format!("{POSTGRES_WORKSPACE_LABEL}={workspace_id}"));
    }
    if let Some(repo) = ctx.repo.as_deref() {
        args.push("--label".to_string());
        args.push(format!(
            "{POSTGRES_REPO_LABEL}={}",
            sanitize_label_value(repo)
        ));
    }
    if let Some(memory) = config.default_memory.as_ref() {
        args.push("--memory".to_string());
        args.push(memory.clone());
    }
    if let Some(cpus) = config.default_cpus.as_ref() {
        args.push("--cpus".to_string());
        args.push(cpus.clone());
    }
    if let Some(pids) = config.default_pids_limit.as_ref() {
        args.push("--pids-limit".to_string());
        args.push(pids.clone());
    }
    args.push(POSTGRES_IMAGE.to_string());

    let output = docker_output(&config.docker_bin, &args).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RunnerError::internal(format!(
            "docker run postgres failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

async fn inspect_container(
    docker_bin: &std::path::Path,
    container_name: &str,
) -> Result<Option<DockerInspect>, RunnerError> {
    let output = docker_output(docker_bin, &["inspect", container_name]).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such object")
            || stderr.contains("No such container")
            || stderr.contains("not found")
        {
            return Ok(None);
        }
        return Err(RunnerError::internal(format!(
            "docker inspect failed for {container_name}: {stderr}"
        )));
    }
    let mut containers: Vec<DockerInspect> = serde_json::from_slice(&output.stdout)
        .map_err(|err| RunnerError::internal(format!("docker inspect JSON parse failed: {err}")))?;
    Ok(containers.pop())
}

async fn postgres_container_names_for_key(
    docker_bin: &std::path::Path,
    key: &str,
) -> Result<Vec<String>, RunnerError> {
    let output = docker_output(
        docker_bin,
        &[
            "ps",
            "-a",
            "--filter",
            &format!("label={OWNER_LABEL}={OWNER_VALUE}"),
            "--filter",
            &format!("label={POSTGRES_SERVICE_LABEL}={POSTGRES_SERVICE_VALUE}"),
            "--filter",
            &format!("label={POSTGRES_KEY_LABEL}={key}"),
            "--format",
            "{{.Names}}",
        ],
    )
    .await?;
    if !output.status.success() {
        return Err(RunnerError::internal(format!(
            "docker ps postgres cleanup query failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

async fn remove_container(
    docker_bin: &std::path::Path,
    container_name: &str,
) -> Result<(), RunnerError> {
    let output = docker_output(docker_bin, &["rm", "-f", container_name]).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RunnerError::internal(format!(
            "docker rm failed for {container_name}: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

async fn docker_logs(
    docker_bin: &std::path::Path,
    container_name: &str,
) -> Result<String, RunnerError> {
    let output = docker_output(docker_bin, &["logs", container_name]).await?;
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

async fn docker_output<I, S>(docker_bin: &std::path::Path, args: I) -> Result<Output, RunnerError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(docker_bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|err| RunnerError::internal(format!("docker command failed: {err}")))
}

fn validate_postgres_container(
    container_name: &str,
    info: &DockerInspect,
    key: &str,
) -> Result<(), RunnerError> {
    let labels = &info.config.labels;
    let valid = labels
        .get(OWNER_LABEL)
        .is_some_and(|value| value == OWNER_VALUE)
        && labels
            .get(POSTGRES_SERVICE_LABEL)
            .is_some_and(|value| value == POSTGRES_SERVICE_VALUE)
        && labels
            .get(POSTGRES_KEY_LABEL)
            .is_some_and(|value| value == key);
    if valid {
        Ok(())
    } else {
        Err(RunnerError::forbidden(format!(
            "container is not this workspace's sulion postgres: {container_name}"
        )))
    }
}

fn postgres_response(
    container_name: &str,
    info: &DockerInspect,
    reused: bool,
) -> Result<PostgresServiceResponse, RunnerError> {
    let env = env_map(&info.config.env);
    let user = env
        .get("POSTGRES_USER")
        .cloned()
        .unwrap_or_else(|| POSTGRES_USER.to_string());
    let password = env.get("POSTGRES_PASSWORD").cloned().ok_or_else(|| {
        RunnerError::internal(format!(
            "postgres container is missing POSTGRES_PASSWORD: {container_name}"
        ))
    })?;
    let database = env
        .get("POSTGRES_DB")
        .cloned()
        .unwrap_or_else(|| POSTGRES_DB.to_string());
    let database_url = format!(
        "postgres://{}:{}@{}:{}/{}?sslmode=disable",
        encode_userinfo(&user),
        encode_userinfo(&password),
        container_name,
        POSTGRES_PORT,
        database
    );
    Ok(PostgresServiceResponse {
        container_name: container_name.to_string(),
        database_url,
        host: container_name.to_string(),
        port: POSTGRES_PORT.parse().unwrap_or(5432),
        user,
        password,
        database,
        reused,
    })
}

fn env_map(values: &[String]) -> HashMap<String, String> {
    values
        .iter()
        .filter_map(|value| {
            let (key, value) = value.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn postgres_key(workspace_id: Option<&str>, cwd: &std::path::Path) -> String {
    if let Some(workspace_id) = workspace_id
        .map(str::trim)
        .filter(|workspace_id| !workspace_id.is_empty())
    {
        return format!("ws-{}", slug_or_hash(workspace_id));
    }
    let cwd = cwd.to_string_lossy();
    format!("cwd-{}", stable_hex_hash(&cwd))
}

fn slug_or_hash(value: &str) -> String {
    let slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() || slug.len() > 48 {
        stable_hex_hash(value)
    } else {
        slug
    }
}

fn sanitize_label_value(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_graphic() && *ch != ',' && *ch != '=')
        .take(63)
        .collect()
}

fn stable_hex_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn encode_userinfo(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => {
                let encoded = format!("%{byte:02X}");
                encoded.chars().collect()
            }
        })
        .collect()
}

enum PostgresCliAction {
    Run {
        mode: PostgresMode,
        command: Vec<OsString>,
    },
    Clean,
}

pub async fn run_postgres_cli(args: &[OsString]) -> anyhow::Result<i32> {
    let action = match parse_postgres_cli_args(args) {
        Ok(action) => action,
        Err(err) => {
            eprintln!("sulion postgres: {err}\n\n{}", postgres_usage());
            return Ok(2);
        }
    };

    match action {
        PostgresCliAction::Clean => run_postgres_clean().await,
        PostgresCliAction::Run { mode, command } => run_with_postgres(mode, command).await,
    }
}

fn parse_postgres_cli_args(args: &[OsString]) -> Result<PostgresCliAction, String> {
    if args.first().and_then(|arg| arg.to_str()) == Some("clean") {
        if args.len() == 1 {
            return Ok(PostgresCliAction::Clean);
        }
        return Err("clean does not accept extra arguments".to_string());
    }

    let mut mode = PostgresMode::Reuse;
    let mut index = 0usize;
    while index < args.len() {
        let Some(arg) = args[index].to_str() else {
            return Err("arguments before -- must be valid UTF-8".to_string());
        };
        match arg {
            "--" => {
                let command = args[index + 1..].to_vec();
                if command.is_empty() {
                    return Err("missing command after --".to_string());
                }
                return Ok(PostgresCliAction::Run { mode, command });
            }
            "--restart" => {
                if mode == PostgresMode::Temp {
                    return Err("--restart and --temp cannot be combined".to_string());
                }
                mode = PostgresMode::Restart;
            }
            "--temp" => {
                if mode == PostgresMode::Restart {
                    return Err("--restart and --temp cannot be combined".to_string());
                }
                mode = PostgresMode::Temp;
            }
            "--help" | "-h" => return Err("usage requested".to_string()),
            other => return Err(format!("unknown argument before --: {other}")),
        }
        index += 1;
    }

    Err("missing -- before command".to_string())
}

fn postgres_usage() -> &'static str {
    "usage:\n  sulion postgres -- <command>\n  sulion postgres --restart -- <command>\n  sulion postgres --temp -- <command>\n  sulion postgres clean"
}

async fn run_postgres_clean() -> anyhow::Result<i32> {
    let request = service_request(PostgresMode::Reuse)?;
    let response =
        match post_runner_json::<_, PostgresCleanResponse>("/v1/services/postgres/clean", &request)
            .await
        {
            Ok(response) => response,
            Err((code, message)) => {
                eprintln!("{message}");
                return Ok(code);
            }
        };
    if response.removed.is_empty() {
        println!("sulion postgres: no stale containers removed");
    } else {
        println!(
            "sulion postgres: removed {} container(s): {}",
            response.removed.len(),
            response.removed.join(", ")
        );
    }
    Ok(0)
}

async fn run_with_postgres(mode: PostgresMode, command: Vec<OsString>) -> anyhow::Result<i32> {
    let request = service_request(mode)?;
    let service = match post_runner_json::<_, PostgresServiceResponse>(
        "/v1/services/postgres/ensure",
        &request,
    )
    .await
    {
        Ok(service) => service,
        Err((code, message)) => {
            eprintln!("{message}");
            return Ok(code);
        }
    };

    let code = run_child_command(command, &service).await?;

    if mode == PostgresMode::Temp {
        let cleanup = PostgresCleanupRequest {
            cwd: request.cwd,
            pty_id: request.pty_id,
            workspace_id: request.workspace_id,
            repo: request.repo,
            container_name: service.container_name,
        };
        if let Err((_, message)) = post_runner_json::<_, PostgresCleanupResponse>(
            "/v1/services/postgres/cleanup",
            &cleanup,
        )
        .await
        {
            eprintln!("{message}");
        }
    }

    Ok(code)
}

fn service_request(mode: PostgresMode) -> anyhow::Result<PostgresServiceRequest> {
    let cwd = std::env::current_dir()
        .context("read current directory")?
        .to_string_lossy()
        .into_owned();
    Ok(PostgresServiceRequest {
        cwd,
        pty_id: std::env::var("SULION_PTY_ID").ok(),
        workspace_id: std::env::var("SULION_WORKSPACE_ID").ok(),
        repo: std::env::var("SULION_REPO_NAME").ok(),
        mode,
    })
}

async fn run_child_command(
    command: Vec<OsString>,
    service: &PostgresServiceResponse,
) -> anyhow::Result<i32> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env("DATABASE_URL", &service.database_url)
        .env("TEST_DATABASE_URL", &service.database_url)
        .env("PGHOST", &service.host)
        .env("PGPORT", service.port.to_string())
        .env("PGUSER", &service.user)
        .env("PGPASSWORD", &service.password)
        .env("PGDATABASE", &service.database)
        .env("PGSSLMODE", "disable")
        .kill_on_drop(false);
    let status = cmd
        .spawn()
        .context("spawn postgres child command")?
        .wait()
        .await?;
    Ok(status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1))
}

async fn post_runner_json<T, R>(path: &str, payload: &T) -> Result<R, (i32, String)>
where
    T: Serialize + ?Sized,
    R: DeserializeOwned,
{
    let runner_url =
        std::env::var("SULION_RUNNER_URL").unwrap_or_else(|_| "http://sulion-runner:8082".into());
    let response = reqwest::Client::new()
        .post(format!("{}{}", runner_url.trim_end_matches('/'), path))
        .json(payload)
        .send()
        .await
        .map_err(|err| (69, format!("sulion postgres: runner request failed: {err}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err((
            64,
            format!("sulion postgres: runner rejected request ({status}): {body}"),
        ));
    }
    response.json::<R>().await.map_err(|err| {
        (
            69,
            format!("sulion postgres: invalid runner response: {err}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_default_restart_temp_and_clean_cli() {
        let action =
            parse_postgres_cli_args(&[OsString::from("--"), OsString::from("cargo")]).unwrap();
        assert!(matches!(
            action,
            PostgresCliAction::Run {
                mode: PostgresMode::Reuse,
                ..
            }
        ));

        let action = parse_postgres_cli_args(&[
            OsString::from("--restart"),
            OsString::from("--"),
            OsString::from("cargo"),
        ])
        .unwrap();
        assert!(matches!(
            action,
            PostgresCliAction::Run {
                mode: PostgresMode::Restart,
                ..
            }
        ));

        let action = parse_postgres_cli_args(&[
            OsString::from("--temp"),
            OsString::from("--"),
            OsString::from("cargo"),
        ])
        .unwrap();
        assert!(matches!(
            action,
            PostgresCliAction::Run {
                mode: PostgresMode::Temp,
                ..
            }
        ));

        assert!(matches!(
            parse_postgres_cli_args(&[OsString::from("clean")]).unwrap(),
            PostgresCliAction::Clean
        ));
    }

    #[test]
    fn rejects_confusing_cli_combinations() {
        assert!(
            parse_postgres_cli_args(&[OsString::from("--restart"), OsString::from("--temp")])
                .is_err()
        );
        assert!(parse_postgres_cli_args(&[OsString::from("--")]).is_err());
        assert!(parse_postgres_cli_args(&[OsString::from("cargo")]).is_err());
    }

    #[test]
    fn workspace_postgres_key_is_stable_and_container_safe() {
        assert_eq!(
            postgres_key(
                Some("18bd1823-5ed0-4160-85c5-c8a7a3e03d7b"),
                &PathBuf::from("/home/dev/repos/sulion")
            ),
            "ws-18bd1823-5ed0-4160-85c5-c8a7a3e03d7b"
        );
        assert_eq!(
            postgres_key(None, &PathBuf::from("/home/dev/repos/sulion")),
            postgres_key(None, &PathBuf::from("/home/dev/repos/sulion"))
        );
    }

    #[test]
    fn database_url_escapes_userinfo() {
        assert_eq!(encode_userinfo("p@ss word"), "p%40ss%20word");
    }
}
