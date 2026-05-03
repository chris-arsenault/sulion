use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

const OWNER_LABEL: &str = "sulion.owner";
const OWNER_VALUE: &str = "sulion";
const PTY_LABEL: &str = "sulion.pty_id";

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub listen: SocketAddr,
    pub docker_bin: PathBuf,
    pub allowed_roots: Vec<PathBuf>,
    pub default_memory: Option<String>,
    pub default_cpus: Option<String>,
    pub default_pids_limit: Option<String>,
}

impl RunnerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen = std::env::var("SULION_RUNNER_LISTEN")
            .unwrap_or_else(|_| "0.0.0.0:8082".to_string())
            .parse()
            .context("invalid SULION_RUNNER_LISTEN")?;
        let docker_bin = PathBuf::from(
            std::env::var("SULION_RUNNER_DOCKER_BIN").unwrap_or_else(|_| "docker".to_string()),
        );
        let allowed_roots = std::env::var("SULION_RUNNER_ALLOWED_ROOTS")
            .unwrap_or_else(|_| "/home/dev/repos".to_string())
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if allowed_roots.is_empty() {
            anyhow::bail!("SULION_RUNNER_ALLOWED_ROOTS must include at least one path");
        }
        Ok(Self {
            listen,
            docker_bin,
            allowed_roots,
            default_memory: env_optional("SULION_RUNNER_DEFAULT_MEMORY")
                .or_else(|| Some("2g".to_string())),
            default_cpus: env_optional("SULION_RUNNER_DEFAULT_CPUS").or_else(|| Some("2".into())),
            default_pids_limit: env_optional("SULION_RUNNER_DEFAULT_PIDS")
                .or_else(|| Some("512".into())),
        })
    }
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Clone)]
pub struct RunnerState {
    config: Arc<RunnerConfig>,
}

impl RunnerState {
    pub fn new(config: RunnerConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Arc::new(config),
        })
    }

    async fn execute(
        &self,
        request: DockerCommandRequest,
    ) -> Result<DockerCommandResponse, RunnerError> {
        let cwd = self.validate_cwd(&request.cwd)?;
        let prepared = prepare_docker_args(&request, &self.config)?;
        ensure_owned_targets(&self.config.docker_bin, prepared.subcommand, &prepared.args).await?;

        let output = Command::new(&self.config.docker_bin)
            .args(&prepared.args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|err| RunnerError::internal(format!("docker command failed: {err}")))?;

        Ok(DockerCommandResponse {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn validate_cwd(&self, cwd: &str) -> Result<PathBuf, RunnerError> {
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(RunnerError::bad_request("cwd must be absolute"));
        }
        let canonical = path
            .canonicalize()
            .map_err(|err| RunnerError::bad_request(format!("invalid cwd: {err}")))?;
        let allowed = self.config.allowed_roots.iter().any(|root| {
            root.canonicalize()
                .map(|root| canonical.starts_with(root))
                .unwrap_or(false)
        });
        if !allowed {
            return Err(RunnerError::forbidden(format!(
                "cwd is outside allowed roots: {}",
                canonical.display()
            )));
        }
        Ok(canonical)
    }
}

pub fn app(state: Arc<RunnerState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/commands/docker", post(run_docker_command))
        .with_state(state)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn run_docker_command(
    axum::extract::State(state): axum::extract::State<Arc<RunnerState>>,
    Json(request): Json<DockerCommandRequest>,
) -> Result<Json<DockerCommandResponse>, RunnerError> {
    state.execute(request).await.map(Json)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DockerCommandRequest {
    pub pty_id: Option<String>,
    pub cwd: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DockerCommandResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
struct RunnerError {
    status: StatusCode,
    message: String,
}

impl RunnerError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl IntoResponse for RunnerError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

struct PreparedDockerArgs {
    subcommand: DockerSubcommand,
    args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DockerSubcommand {
    Build,
    Run,
    Ps,
    Images,
    Pull,
    Logs,
    Stop,
    Rm,
    Inspect,
    Version,
    Help,
}

fn prepare_docker_args(
    request: &DockerCommandRequest,
    config: &RunnerConfig,
) -> Result<PreparedDockerArgs, RunnerError> {
    if request.argv.is_empty() {
        return Err(RunnerError::bad_request("docker command is required"));
    }
    if request
        .argv
        .iter()
        .any(|arg| arg == "--help" || arg == "-h")
    {
        return Ok(PreparedDockerArgs {
            subcommand: DockerSubcommand::Help,
            args: request.argv.clone(),
        });
    }

    let command = request.argv[0].as_str();
    match command {
        "--version" | "version" => Ok(PreparedDockerArgs {
            subcommand: DockerSubcommand::Version,
            args: request.argv.clone(),
        }),
        "build" => prepare_build_args(request, config),
        "run" => prepare_run_args(request, config),
        "ps" => prepare_ps_args(request),
        "images" => Ok(simple_args(request, DockerSubcommand::Images)),
        "pull" => Ok(simple_args(request, DockerSubcommand::Pull)),
        "logs" => prepare_logs_args(request),
        "stop" => Ok(simple_args(request, DockerSubcommand::Stop)),
        "rm" => Ok(simple_args(request, DockerSubcommand::Rm)),
        "inspect" => Ok(simple_args(request, DockerSubcommand::Inspect)),
        _ => Err(RunnerError::forbidden(format!(
            "docker subcommand is not supported: {command}"
        ))),
    }
}

fn simple_args(request: &DockerCommandRequest, subcommand: DockerSubcommand) -> PreparedDockerArgs {
    PreparedDockerArgs {
        subcommand,
        args: request.argv.clone(),
    }
}

fn prepare_build_args(
    request: &DockerCommandRequest,
    _config: &RunnerConfig,
) -> Result<PreparedDockerArgs, RunnerError> {
    deny_dangerous_flags(&request.argv[1..], FlagPolicy::Build)?;
    let mut args = vec![
        "build".to_string(),
        "--label".to_string(),
        format!("{OWNER_LABEL}={OWNER_VALUE}"),
        "--label".to_string(),
        format!("{PTY_LABEL}={}", pty_label_value(request)),
    ];
    args.extend_from_slice(&request.argv[1..]);
    Ok(PreparedDockerArgs {
        subcommand: DockerSubcommand::Build,
        args,
    })
}

fn prepare_run_args(
    request: &DockerCommandRequest,
    config: &RunnerConfig,
) -> Result<PreparedDockerArgs, RunnerError> {
    deny_dangerous_flags(&request.argv[1..], FlagPolicy::Run)?;
    let mut args = vec![
        "run".to_string(),
        "--label".to_string(),
        format!("{OWNER_LABEL}={OWNER_VALUE}"),
        "--label".to_string(),
        format!("{PTY_LABEL}={}", pty_label_value(request)),
    ];
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
    args.extend_from_slice(&request.argv[1..]);
    Ok(PreparedDockerArgs {
        subcommand: DockerSubcommand::Run,
        args,
    })
}

fn prepare_ps_args(request: &DockerCommandRequest) -> Result<PreparedDockerArgs, RunnerError> {
    let mut args = vec![
        "ps".to_string(),
        "--filter".to_string(),
        format!("label={OWNER_LABEL}={OWNER_VALUE}"),
    ];
    args.extend_from_slice(&request.argv[1..]);
    Ok(PreparedDockerArgs {
        subcommand: DockerSubcommand::Ps,
        args,
    })
}

fn prepare_logs_args(request: &DockerCommandRequest) -> Result<PreparedDockerArgs, RunnerError> {
    deny_log_streaming_flags(&request.argv[1..])?;
    Ok(simple_args(request, DockerSubcommand::Logs))
}

fn pty_label_value(request: &DockerCommandRequest) -> &str {
    request.pty_id.as_deref().unwrap_or("unknown")
}

#[derive(Clone, Copy)]
enum FlagPolicy {
    Build,
    Run,
}

fn deny_dangerous_flags(args: &[String], policy: FlagPolicy) -> Result<(), RunnerError> {
    let denied = match policy {
        FlagPolicy::Build => [
            "--allow",
            "--build-context",
            "--iidfile",
            "--network",
            "--metadata-file",
            "--output",
            "--secret",
            "--ssh",
            "--label",
            "-o",
        ]
        .as_slice(),
        FlagPolicy::Run => [
            "--add-host",
            "--cap-add",
            "--cgroupns",
            "--cpu-period",
            "--cpu-quota",
            "--cpu-shares",
            "--cpus",
            "--cpuset-cpus",
            "--device",
            "--group-add",
            "--ipc",
            "--label",
            "--memory",
            "--memory-reservation",
            "--memory-swap",
            "--mount",
            "--network",
            "--net",
            "--oom-kill-disable",
            "--pid",
            "--pids-limit",
            "--privileged",
            "--restart",
            "--security-opt",
            "--storage-opt",
            "--tmpfs",
            "--userns",
            "--uts",
            "--volume",
            "--volumes-from",
            "-m",
            "-v",
        ]
        .as_slice(),
    };

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if matches!(policy, FlagPolicy::Run)
            && matches!(arg, "-i" | "-t" | "-it" | "-ti" | "--interactive" | "--tty")
        {
            return Err(RunnerError::forbidden(
                "interactive docker runs are not supported through sulion-runner",
            ));
        }
        for flag in denied {
            if arg == *flag
                || arg.starts_with(&format!("{flag}="))
                || (flag.len() == 2 && arg.starts_with(flag) && arg.len() > 2)
            {
                return Err(RunnerError::forbidden(format!(
                    "docker flag is not allowed through sulion-runner: {flag}"
                )));
            }
        }
        if matches!(
            arg,
            "--pid=host"
                | "--network=host"
                | "--net=host"
                | "--ipc=host"
                | "--uts=host"
                | "--cgroupns=host"
        ) {
            return Err(RunnerError::forbidden(format!(
                "host namespace flag is not allowed: {arg}"
            )));
        }
        index += 1;
    }
    Ok(())
}

fn deny_log_streaming_flags(args: &[String]) -> Result<(), RunnerError> {
    for arg in args {
        if matches!(arg.as_str(), "-f" | "--follow") || arg.starts_with("--follow=") {
            return Err(RunnerError::forbidden(
                "streaming docker logs are not supported through sulion-runner",
            ));
        }
    }
    Ok(())
}

async fn ensure_owned_targets(
    docker_bin: &Path,
    subcommand: DockerSubcommand,
    args: &[String],
) -> Result<(), RunnerError> {
    if !matches!(
        subcommand,
        DockerSubcommand::Logs
            | DockerSubcommand::Stop
            | DockerSubcommand::Rm
            | DockerSubcommand::Inspect
    ) {
        return Ok(());
    }
    let targets = container_targets(args);
    if targets.is_empty() {
        return Err(RunnerError::bad_request(
            "container command requires at least one target",
        ));
    }
    for target in targets {
        ensure_target_owned(docker_bin, &target).await?;
    }
    Ok(())
}

fn container_targets(args: &[String]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            targets.extend(args[index + 1..].iter().cloned());
            break;
        }
        if arg.starts_with("--") {
            if flag_takes_value(arg) && !arg.contains('=') {
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        targets.push(arg.clone());
        index += 1;
    }
    targets
}

fn flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--filter" | "--format" | "--since" | "--tail" | "--until" | "--time"
    )
}

async fn ensure_target_owned(docker_bin: &Path, target: &str) -> Result<(), RunnerError> {
    let output = Command::new(docker_bin)
        .args([
            "inspect",
            "--format",
            &format!("{{{{ index .Config.Labels {:?} }}}}", OWNER_LABEL),
            target,
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|err| RunnerError::internal(format!("docker inspect failed: {err}")))?;
    if !output.status.success() {
        return Err(RunnerError::forbidden(format!(
            "container is not accessible through sulion-runner: {target}"
        )));
    }
    let owner = String::from_utf8_lossy(&output.stdout);
    if owner.trim() != OWNER_VALUE {
        return Err(RunnerError::forbidden(format!(
            "container is not owned by sulion: {target}"
        )));
    }
    Ok(())
}

pub async fn run_client(args: &[OsString]) -> anyhow::Result<i32> {
    let argv = args
        .iter()
        .map(|arg| {
            arg.clone()
                .into_string()
                .map_err(|_| anyhow::anyhow!("docker arguments must be valid UTF-8"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let runner_url =
        std::env::var("SULION_RUNNER_URL").unwrap_or_else(|_| "http://sulion-runner:8082".into());
    let cwd = std::env::current_dir().context("read current directory")?;
    let request = DockerCommandRequest {
        pty_id: std::env::var("SULION_PTY_ID").ok(),
        cwd: cwd.to_string_lossy().into_owned(),
        argv,
    };
    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1/commands/docker",
            runner_url.trim_end_matches('/')
        ))
        .json(&request)
        .send()
        .await;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            eprintln!("docker: sulion-runner rejected request ({status}): {body}");
            return Ok(64);
        }
        Err(err) => {
            eprintln!("docker: sulion-runner request failed: {err}");
            return Ok(69);
        }
    };
    let payload = response
        .json::<DockerCommandResponse>()
        .await
        .context("invalid sulion-runner response")?;
    print!("{}", payload.stdout);
    eprint!("{}", payload.stderr);
    Ok(payload.exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RunnerConfig {
        RunnerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            docker_bin: "docker".into(),
            allowed_roots: vec!["/home/dev/repos".into()],
            default_memory: Some("1g".into()),
            default_cpus: Some("1".into()),
            default_pids_limit: Some("128".into()),
        }
    }

    #[test]
    fn run_injects_labels_and_limits() {
        let request = DockerCommandRequest {
            pty_id: Some("pty-1".into()),
            cwd: "/home/dev/repos/app".into(),
            argv: vec!["run".into(), "--rm".into(), "alpine".into(), "true".into()],
        };
        let prepared = prepare_docker_args(&request, &test_config()).unwrap();
        assert_eq!(prepared.subcommand, DockerSubcommand::Run);
        assert!(prepared
            .args
            .contains(&format!("{OWNER_LABEL}={OWNER_VALUE}")));
        assert!(prepared.args.contains(&format!("{PTY_LABEL}=pty-1")));
        assert!(prepared
            .args
            .windows(2)
            .any(|pair| pair == ["--memory", "1g"]));
        assert!(prepared.args.windows(2).any(|pair| pair == ["--cpus", "1"]));
        assert!(prepared
            .args
            .windows(2)
            .any(|pair| pair == ["--pids-limit", "128"]));
    }

    #[test]
    fn run_denies_privileged_and_mounts() {
        for args in [
            vec!["run", "--privileged", "alpine"],
            vec!["run", "-v", "/:/host", "alpine"],
            vec!["run", "-v/:/host", "alpine"],
            vec!["run", "--mount", "type=bind,src=/,dst=/host", "alpine"],
            vec!["run", "--network=host", "alpine"],
            vec!["run", "-it", "alpine"],
        ] {
            let request = DockerCommandRequest {
                pty_id: None,
                cwd: "/home/dev/repos/app".into(),
                argv: args.into_iter().map(str::to_string).collect(),
            };
            assert!(prepare_docker_args(&request, &test_config()).is_err());
        }
    }

    #[test]
    fn run_denies_resource_limit_overrides() {
        for args in [
            vec!["run", "--memory", "10g", "alpine"],
            vec!["run", "-m10g", "alpine"],
            vec!["run", "--cpus=32", "alpine"],
            vec!["run", "--pids-limit", "-1", "alpine"],
            vec!["run", "--restart=always", "alpine"],
        ] {
            let request = DockerCommandRequest {
                pty_id: None,
                cwd: "/home/dev/repos/app".into(),
                argv: args.into_iter().map(str::to_string).collect(),
            };
            assert!(prepare_docker_args(&request, &test_config()).is_err());
        }
    }

    #[test]
    fn build_injects_labels_and_preserves_context() {
        let request = DockerCommandRequest {
            pty_id: Some("pty-2".into()),
            cwd: "/home/dev/repos/app".into(),
            argv: vec!["build".into(), "-t".into(), "local/app".into(), ".".into()],
        };
        let prepared = prepare_docker_args(&request, &test_config()).unwrap();
        assert_eq!(prepared.subcommand, DockerSubcommand::Build);
        assert!(prepared
            .args
            .contains(&format!("{OWNER_LABEL}={OWNER_VALUE}")));
        assert!(prepared.args.contains(&".".to_string()));
    }

    #[test]
    fn build_denies_host_output_flags() {
        for args in [
            vec!["build", "--output", "type=local,dest=/tmp/out", "."],
            vec!["build", "-o", "/tmp/out", "."],
            vec!["build", "--iidfile=/tmp/iid", "."],
        ] {
            let request = DockerCommandRequest {
                pty_id: None,
                cwd: "/home/dev/repos/app".into(),
                argv: args.into_iter().map(str::to_string).collect(),
            };
            assert!(prepare_docker_args(&request, &test_config()).is_err());
        }
    }

    #[test]
    fn logs_denies_follow_mode() {
        let request = DockerCommandRequest {
            pty_id: None,
            cwd: "/home/dev/repos/app".into(),
            argv: vec!["logs".into(), "--follow".into(), "container".into()],
        };
        assert!(prepare_docker_args(&request, &test_config()).is_err());
    }

    #[test]
    fn ps_is_label_filtered() {
        let request = DockerCommandRequest {
            pty_id: None,
            cwd: "/home/dev/repos/app".into(),
            argv: vec!["ps".into(), "-a".into()],
        };
        let prepared = prepare_docker_args(&request, &test_config()).unwrap();
        assert_eq!(
            prepared.args,
            vec!["ps", "--filter", "label=sulion.owner=sulion", "-a"]
        );
    }
}
