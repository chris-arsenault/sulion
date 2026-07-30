use std::ffi::OsString;

use anyhow::{anyhow, Context};
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
use url::Url;

use crate::cli_http::{
    agent_session_id, bearer_headers, build_url, env_optional, env_required, infer_repo,
    insert_header,
};
use crate::code_intel::help::{help_response, HELP_TEXT};

mod output;
#[cfg(test)]
mod tests;

use output::{print_json, print_text};

const DEFAULT_BUDGET: &str = "normal";

pub async fn run(args: &[OsString]) -> anyhow::Result<i32> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("code arguments must be valid UTF-8"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let invocation = match CliInvocation::parse(&args) {
        Ok(invocation) => invocation,
        Err(err) => {
            eprintln!("sulion-code: {}", err.message);
            eprintln!("next: {}", err.next);
            return Ok(64);
        }
    };
    if matches!(invocation.command, CodeCommand::Help) {
        if invocation.json {
            print_json(&json!(help_response(1)));
        } else {
            println!("{HELP_TEXT}");
        }
        return Ok(0);
    }
    let env = match CodeCliEnv::from_env() {
        Ok(env) => env,
        Err(err) => {
            eprintln!("sulion-code: {err}");
            eprintln!("next: sulion-code help");
            return Ok(65);
        }
    };
    let request = invocation.command.request(&env.cwd, &invocation.budget);
    // Trusts the pinned control certificate in addition to public roots.
    let client = crate::node_protocol::tls::control_http_client();
    match request_json(&client, &env, request).await {
        Ok(body) => {
            if invocation.json {
                print_json(&body);
            } else {
                print_text(&invocation.command, &body);
            }
            Ok(0)
        }
        Err(err) => {
            eprintln!("sulion-code: {err}");
            eprintln!("next: sulion-code status");
            Ok(66)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliInvocation {
    json: bool,
    budget: String,
    command: CodeCommand,
}

impl CliInvocation {
    fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut json = false;
        let mut budget = DEFAULT_BUDGET.to_string();
        let mut positionals = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            match arg.as_str() {
                "--json" => json = true,
                "--budget" => {
                    index += 1;
                    let Some(value) = args.get(index) else {
                        return Err(CliError::usage("--budget requires small, normal, or large"));
                    };
                    budget = parse_budget(value)?;
                }
                "--" => {
                    positionals.extend(args[index + 1..].iter().cloned());
                    break;
                }
                "-h" | "--help" => positionals.push("help".to_string()),
                value if value.starts_with("--budget=") => {
                    budget = parse_budget(value.trim_start_matches("--budget="))?;
                }
                value if value.starts_with("--") => {
                    return Err(CliError::usage(format!("unknown option: {value}")));
                }
                value => positionals.push(value.to_string()),
            }
            index += 1;
        }
        let command = CodeCommand::parse(&positionals)?;
        Ok(Self {
            json,
            budget,
            command,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CodeCommand {
    Help,
    Status,
    IndexStatus,
    Refresh {
        path: Option<String>,
    },
    Outline {
        path: Option<String>,
    },
    Find {
        query: String,
    },
    Def {
        target: String,
    },
    Refs {
        target: String,
    },
    Search {
        lang: String,
        pattern: String,
        path: Option<String>,
    },
    Patch {
        lang: String,
        pattern: String,
        rewrite: String,
        path: Option<String>,
    },
    Pack {
        target: String,
    },
}

impl CodeCommand {
    fn parse(positionals: &[String]) -> Result<Self, CliError> {
        let Some(command) = positionals.first().map(String::as_str) else {
            return Err(CliError::usage("missing command"));
        };
        let args = &positionals[1..];
        match command {
            "help" => no_args(args, Self::Help, "sulion-code help"),
            "status" => no_args(args, Self::Status, "sulion-code status"),
            "index-status" => no_args(args, Self::IndexStatus, "sulion-code index-status"),
            "refresh" => optional_path(args, |path| Self::Refresh { path }, "refresh"),
            "outline" => optional_path(args, |path| Self::Outline { path }, "outline"),
            "find" => one_arg(
                args,
                |query| Self::Find { query },
                "sulion-code find <symbol-or-name>",
            ),
            "def" => one_arg(
                args,
                |target| Self::Def { target },
                "sulion-code def <path:line[:col] | symbol-id>",
            ),
            "refs" => one_arg(
                args,
                |target| Self::Refs { target },
                "sulion-code refs <path:line[:col] | symbol-id>",
            ),
            "search" => match args {
                [lang, pattern] => Ok(Self::Search {
                    lang: lang.clone(),
                    pattern: pattern.clone(),
                    path: None,
                }),
                [lang, pattern, path] => Ok(Self::Search {
                    lang: lang.clone(),
                    pattern: pattern.clone(),
                    path: Some(path.clone()),
                }),
                _ => Err(CliError::usage(
                    "usage: sulion-code search <lang> <pattern> [path]",
                )),
            },
            "patch" => match args {
                [lang, pattern, rewrite] => Ok(Self::Patch {
                    lang: lang.clone(),
                    pattern: pattern.clone(),
                    rewrite: rewrite.clone(),
                    path: None,
                }),
                [lang, pattern, rewrite, path] => Ok(Self::Patch {
                    lang: lang.clone(),
                    pattern: pattern.clone(),
                    rewrite: rewrite.clone(),
                    path: Some(path.clone()),
                }),
                _ => Err(CliError::usage(
                    "usage: sulion-code patch <lang> <pattern> <rewrite> [path]",
                )),
            },
            "pack" => one_arg(
                args,
                |target| Self::Pack { target },
                "sulion-code pack <path:line-line | symbol-id>",
            ),
            other => Err(CliError::usage(format!("unknown command: {other}"))),
        }
    }

    fn request(&self, cwd: &str, budget: &str) -> CodeRequest {
        match self {
            Self::Help => unreachable!("help is handled locally"),
            Self::Status => CodeRequest::get("/v1/status", vec![("cwd", cwd.to_string())]),
            Self::IndexStatus => {
                CodeRequest::get("/v1/index/status", vec![("cwd", cwd.to_string())])
            }
            Self::Refresh { path } => CodeRequest::post_query(
                "/v1/refresh",
                query_with_path(cwd, budget, path.as_deref()),
            ),
            Self::Outline { path } => {
                CodeRequest::get("/v1/outline", query_with_path(cwd, budget, path.as_deref()))
            }
            Self::Find { query } => CodeRequest::get(
                "/v1/find",
                vec![
                    ("cwd", cwd.to_string()),
                    ("budget", budget.to_string()),
                    ("q", query.clone()),
                ],
            ),
            Self::Def { target } => CodeRequest::get(
                "/v1/def",
                vec![
                    ("cwd", cwd.to_string()),
                    ("budget", budget.to_string()),
                    ("target", target.clone()),
                ],
            ),
            Self::Refs { target } => CodeRequest::get(
                "/v1/refs",
                vec![
                    ("cwd", cwd.to_string()),
                    ("budget", budget.to_string()),
                    ("target", target.clone()),
                ],
            ),
            Self::Search {
                lang,
                pattern,
                path,
            } => {
                let mut query = query_with_path(cwd, budget, path.as_deref());
                query.push(("lang", lang.clone()));
                query.push(("pattern", pattern.clone()));
                CodeRequest::get("/v1/search", query)
            }
            Self::Patch {
                lang,
                pattern,
                rewrite,
                path,
            } => CodeRequest {
                method: "POST",
                path: "/v1/patch",
                query: Vec::new(),
                body: Some(json!({
                    "cwd": cwd,
                    "budget": budget,
                    "lang": lang,
                    "pattern": pattern,
                    "rewrite": rewrite,
                    "path": path,
                })),
            },
            Self::Pack { target } => CodeRequest::get(
                "/v1/pack",
                vec![
                    ("cwd", cwd.to_string()),
                    ("budget", budget.to_string()),
                    ("target", target.clone()),
                ],
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct CodeRequest {
    method: &'static str,
    path: &'static str,
    query: Vec<(&'static str, String)>,
    body: Option<Value>,
}

impl CodeRequest {
    fn get(path: &'static str, query: Vec<(&'static str, String)>) -> Self {
        Self {
            method: "GET",
            path,
            query,
            body: None,
        }
    }

    fn post_query(path: &'static str, query: Vec<(&'static str, String)>) -> Self {
        Self {
            method: "POST",
            path,
            query,
            body: None,
        }
    }
}

#[derive(Debug)]
struct CliError {
    message: String,
    next: &'static str,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            next: "sulion-code help",
        }
    }
}

#[derive(Debug, Clone)]
struct CodeCliEnv {
    base_url: String,
    token: String,
    cwd: String,
    repo: Option<String>,
    pty_id: Option<String>,
    workspace_id: Option<String>,
    base_sha: Option<String>,
    agent_session_id: Option<String>,
}

impl CodeCliEnv {
    fn from_env() -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()
            .context("read current directory")?
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            base_url: env_required("SULION_CODE_INTEL_URL")?,
            token: env_required("SULION_CODE_INTEL_TOKEN")?,
            repo: env_optional("SULION_REPO_NAME").or_else(|| infer_repo(&cwd)),
            cwd,
            pty_id: env_optional("SULION_PTY_ID"),
            workspace_id: env_optional("SULION_WORKSPACE_ID"),
            base_sha: env_optional("SULION_BASE_SHA"),
            agent_session_id: agent_session_id(),
        })
    }

    fn headers(&self) -> anyhow::Result<HeaderMap> {
        let mut headers = bearer_headers(&self.token, "invalid code-intel token header")?;
        insert_header(&mut headers, "x-sulion-cwd", Some(&self.cwd))?;
        insert_header(&mut headers, "x-sulion-repo", self.repo.as_deref())?;
        insert_header(&mut headers, "x-sulion-pty-id", self.pty_id.as_deref())?;
        insert_header(
            &mut headers,
            "x-sulion-workspace-id",
            self.workspace_id.as_deref(),
        )?;
        insert_header(&mut headers, "x-sulion-base-sha", self.base_sha.as_deref())?;
        insert_header(
            &mut headers,
            "x-sulion-agent-session-id",
            self.agent_session_id.as_deref(),
        )?;
        Ok(headers)
    }

    fn url(&self, path: &str, pairs: &[(&str, String)]) -> anyhow::Result<Url> {
        build_url(&self.base_url, path, pairs, "invalid SULION_CODE_INTEL_URL")
    }
}

async fn request_json(
    client: &reqwest::Client,
    env: &CodeCliEnv,
    request: CodeRequest,
) -> anyhow::Result<Value> {
    let url = env.url(request.path, &request.query)?;
    let builder = match request.method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        _ => return Err(anyhow!("unsupported method: {}", request.method)),
    }
    .headers(env.headers()?);
    let builder = if let Some(body) = request.body {
        builder.json(&body)
    } else {
        builder
    };
    let response = builder.send().await.context("code-intel request failed")?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("code-intel request failed ({status}): {text}"));
    }
    serde_json::from_str(&text).context("code-intel response was not JSON")
}

fn query_with_path(cwd: &str, budget: &str, path: Option<&str>) -> Vec<(&'static str, String)> {
    let mut query = vec![("cwd", cwd.to_string()), ("budget", budget.to_string())];
    if let Some(path) = path {
        query.push(("path", path.to_string()));
    }
    query
}

fn no_args(
    args: &[String],
    command: CodeCommand,
    usage: &'static str,
) -> Result<CodeCommand, CliError> {
    if args.is_empty() {
        Ok(command)
    } else {
        Err(CliError::usage(format!("usage: {usage}")))
    }
}

fn optional_path(
    args: &[String],
    build: impl FnOnce(Option<String>) -> CodeCommand,
    command: &'static str,
) -> Result<CodeCommand, CliError> {
    match args {
        [] => Ok(build(None)),
        [path] => Ok(build(Some(path.clone()))),
        _ => Err(CliError::usage(format!(
            "usage: sulion-code {command} [path]"
        ))),
    }
}

fn one_arg(
    args: &[String],
    build: impl FnOnce(String) -> CodeCommand,
    usage: &'static str,
) -> Result<CodeCommand, CliError> {
    match args {
        [value] => Ok(build(value.clone())),
        _ => Err(CliError::usage(format!("usage: {usage}"))),
    }
}

fn parse_budget(value: &str) -> Result<String, CliError> {
    match value {
        "small" | "normal" | "large" => Ok(value.to_string()),
        _ => Err(CliError::usage(
            "--budget must be one of small, normal, or large",
        )),
    }
}
