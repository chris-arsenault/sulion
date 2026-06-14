use std::ffi::OsString;
use std::path::Path;

use anyhow::{anyhow, Context};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use url::Url;

use crate::code_intel::help::{help_response, HELP_TEXT};

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
    let client = reqwest::Client::new();
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
            agent_session_id: env_optional("SULION_AGENT_SESSION_ID")
                .or_else(|| env_optional("SULION_CLAUDE_SESSION_ID"))
                .or_else(|| env_optional("CODEX_SESSION_ID")),
        })
    }

    fn headers(&self) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .context("invalid code-intel token header")?,
        );
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
        let mut url = Url::parse(self.base_url.trim_end_matches('/'))
            .context("invalid SULION_CODE_INTEL_URL")?
            .join(path.trim_start_matches('/'))?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in pairs {
                if !value.trim().is_empty() {
                    query.append_pair(key, value);
                }
            }
        }
        Ok(url)
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

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        headers.insert(name, HeaderValue::from_str(value)?);
    }
    Ok(())
}

fn env_required(key: &str) -> anyhow::Result<String> {
    std::env::var(key)
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("{key} is not set"))
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn infer_repo(cwd: &str) -> Option<String> {
    let path = Path::new(cwd);
    for prefix in ["/home/dev/repos", "/home/dev/workspaces"] {
        if let Ok(rest) = path.strip_prefix(prefix) {
            if let Some(component) = rest.components().next() {
                let repo = component.as_os_str().to_string_lossy();
                if !repo.is_empty() {
                    return Some(repo.into_owned());
                }
            }
        }
    }
    None
}

fn print_text(command: &CodeCommand, body: &Value) {
    match command {
        CodeCommand::Status => print_status(body),
        CodeCommand::IndexStatus => print_index_status(body),
        CodeCommand::Refresh { .. } => print_refresh(body),
        CodeCommand::Outline { .. } | CodeCommand::Find { .. } | CodeCommand::Def { .. } => {
            print_symbol_results(body)
        }
        CodeCommand::Refs { .. } => print_reference_results(body),
        CodeCommand::Search { .. } => print_search_results(body),
        CodeCommand::Patch { .. } => print_patch(body),
        CodeCommand::Pack { .. } => print_pack(body),
        CodeCommand::Help => unreachable!("help is handled locally"),
    }
}

fn print_index_status(body: &Value) {
    print_warnings(body);
    println!(
        "root: {} {} {}",
        text(&body["root"], "kind"),
        text(&body["root"], "name"),
        text(&body["root"], "path")
    );
    println!(
        "state: freshness={} confidence={}",
        text(body, "freshness"),
        text(body, "confidence")
    );
    print_index_summary(&body["index"]);
    let latest_job = &body["index"]["latest_job"];
    if latest_job.is_object() {
        println!(
            "latest-job: status={} trigger={} seen={} indexed={} failed={}",
            text(latest_job, "status"),
            text(latest_job, "trigger"),
            number(latest_job, "files_seen"),
            number(latest_job, "files_indexed"),
            number(latest_job, "files_failed")
        );
    }
}

fn print_status(body: &Value) {
    print_warnings(body);
    println!(
        "root: {} {} {}",
        text(&body["root"], "kind"),
        text(&body["root"], "name"),
        text(&body["root"], "path")
    );
    println!(
        "state: freshness={} confidence={}",
        text(body, "freshness"),
        text(body, "confidence")
    );
    print_index_summary(&body["index"]);
    let semantic = &body["semantic"];
    println!(
        "semantic: available={} fallback={} timeout_ms={}",
        bool_text(semantic, "available"),
        text(semantic, "fallback"),
        number(semantic, "timeout_ms")
    );
    print_string_list("languages", &body["supported_languages"]);
    print_string_list("next", &body["examples"]);
}

fn print_index_summary(index: &Value) {
    println!(
        "index: files={} symbols={} pending={} deleted={} partial={} failed={}",
        number(index, "file_count"),
        number(index, "symbol_count"),
        number(index, "pending_file_count"),
        number(index, "deleted_file_count"),
        number(index, "partial_file_count"),
        number(index, "failed_file_count")
    );
}

fn print_refresh(body: &Value) {
    print_warnings(body);
    println!(
        "refresh: root={} path={} freshness={} confidence={}",
        text(&body["root"], "path"),
        optional_text(body, "path").unwrap_or("."),
        text(body, "freshness"),
        text(body, "confidence")
    );
    let stats = &body["stats"];
    println!(
        "stats: seen={} marked_pending={} deleted={}",
        number(stats, "files_seen"),
        number(stats, "files_marked_pending"),
        number(stats, "files_deleted")
    );
}

fn print_symbol_results(body: &Value) {
    print_envelope(body);
    for result in results(body) {
        let range = format_range(&result["range"]);
        println!(
            "{} {} {} {} {}",
            range,
            text(result, "confidence"),
            text(result, "freshness"),
            text(result, "kind"),
            text(result, "qualified_name")
        );
        if let Some(signature) = optional_text(result, "signature") {
            println!("  signature: {signature}");
        }
        println!("  id: {}", text(result, "id"));
        if !result["body_range"].is_null() {
            println!("  body: {}", format_range(&result["body_range"]));
        }
        println!("  next: sulion-code pack {}", text(result, "id"));
    }
}

fn print_reference_results(body: &Value) {
    print_envelope(body);
    for result in results(body) {
        println!(
            "{} {} {} {} {}",
            format_range(&result["range"]),
            text(result, "confidence"),
            text(result, "freshness"),
            text(result, "reference_kind"),
            text(result, "referenced_name")
        );
        if let Some(symbol_id) = optional_text(result, "symbol_id") {
            println!("  symbol: {symbol_id}");
        }
    }
}

fn print_search_results(body: &Value) {
    print_envelope(body);
    for result in results(body) {
        println!(
            "{} {} {} {}",
            format_range(&result["range"]),
            text(result, "confidence"),
            text(result, "freshness"),
            text(result, "kind")
        );
        if let Some(text) = optional_text(result, "text") {
            println!("  match: {}", compact(text));
        }
        if let Some(captures) = result["captures"].as_array() {
            for capture in captures.iter().take(4) {
                println!(
                    "  capture ${}: {} {}",
                    text(capture, "name"),
                    format_range(&capture["range"]),
                    compact(optional_text(capture, "text").unwrap_or(""))
                );
            }
        }
    }
}

fn print_patch(body: &Value) {
    print_warnings(body);
    println!(
        "patch: matches={} applied={} truncated={}",
        number(body, "matches"),
        bool_text(body, "applied"),
        bool_text(body, "truncated")
    );
    if let Some(files) = body["files"].as_array() {
        for file in files {
            println!(
                "file: {} matches={}",
                text(file, "path"),
                number(file, "matches")
            );
        }
    }
    let diff = optional_text(body, "diff").unwrap_or("");
    if diff.is_empty() {
        println!("diff: <empty>");
    } else {
        print!("{diff}");
        if !diff.ends_with('\n') {
            println!();
        }
    }
}

fn print_pack(body: &Value) {
    print_warnings(body);
    let bundle = &body["bundle"];
    let target = &bundle["target"];
    println!(
        "pack: {} {} budget={} confidence={} freshness={}",
        text(target, "kind"),
        format_range(&target["range"]),
        text(body, "budget"),
        text(body, "confidence"),
        text(body, "freshness")
    );
    let primary = &bundle["primary"];
    if primary.is_object() {
        println!(
            "primary: {} {} {}",
            text(primary, "kind"),
            text(primary, "qualified_name"),
            format_range(&primary["range"])
        );
    }
    println!("excerpt: {}", format_range(&bundle["excerpt"]["range"]));
    if let Some(excerpt) = optional_text(&bundle["excerpt"], "text") {
        if !excerpt.is_empty() {
            println!("{excerpt}");
        }
    }
    print_count("containers", &bundle["containers"]);
    print_count("imports", &bundle["imports"]);
    print_count("references", &bundle["references"]);
    print_count("nearby_tests", &bundle["nearby_tests"]);
}

fn print_envelope(body: &Value) {
    print_warnings(body);
    println!(
        "{}: root={} freshness={} confidence={} truncated={}",
        text(body, "command"),
        text(&body["root"], "path"),
        text(body, "freshness"),
        text(body, "confidence"),
        bool_text(body, "truncated")
    );
    if results(body).is_empty() {
        println!("results: none");
    }
}

fn print_warnings(body: &Value) {
    if let Some(warnings) = body["warnings"].as_array() {
        for warning in warnings.iter().filter_map(Value::as_str) {
            eprintln!("warning: {warning}");
        }
    }
}

fn results(body: &Value) -> &[Value] {
    body["results"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn format_range(value: &Value) -> String {
    let path = text(value, "path");
    let start_line = number(value, "start_line");
    let start_col = number(value, "start_col");
    let end_line = number(value, "end_line");
    let end_col = number(value, "end_col");
    format!("{path}:{start_line}:{start_col}-{end_line}:{end_col}")
}

fn print_string_list(label: &str, value: &Value) {
    let items = value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !items.is_empty() {
        println!("{label}: {items}");
    }
}

fn print_count(label: &str, value: &Value) {
    let count = value.as_array().map(Vec::len).unwrap_or_default();
    println!("{label}: {count}");
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    optional_text(value, key).unwrap_or("-")
}

fn optional_text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn bool_text<'a>(value: &'a Value, key: &str) -> &'a str {
    if value.get(key).and_then(Value::as_bool).unwrap_or(false) {
        "true"
    } else {
        "false"
    }
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_global_options_around_command() {
        let parsed = CliInvocation::parse(&args(&[
            "outline",
            "backend/src",
            "--budget",
            "small",
            "--json",
        ]))
        .unwrap();

        assert!(parsed.json);
        assert_eq!(parsed.budget, "small");
        assert_eq!(
            parsed.command,
            CodeCommand::Outline {
                path: Some("backend/src".to_string())
            }
        );
    }

    #[test]
    fn rejects_non_canonical_options() {
        let err = CliInvocation::parse(&args(&["status", "--repo", "sulion"])).unwrap_err();

        assert!(err.message.contains("unknown option"));
        assert_eq!(err.next, "sulion-code help");
    }

    #[test]
    fn patch_request_is_diff_only_command_shape() {
        let parsed = CliInvocation::parse(&args(&[
            "patch",
            "rust",
            "foo($A)",
            "bar($A)",
            "backend/src",
            "--budget=large",
        ]))
        .unwrap();
        let request = parsed
            .command
            .request("/home/dev/repos/sulion", &parsed.budget);

        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/patch");
        assert_eq!(request.query.len(), 0);
        let body = request.body.unwrap();
        assert_eq!(body["cwd"], "/home/dev/repos/sulion");
        assert_eq!(body["lang"], "rust");
        assert_eq!(body["rewrite"], "bar($A)");
        assert_eq!(body["path"], "backend/src");
        assert_eq!(body["budget"], "large");
    }

    #[test]
    fn help_does_not_require_service_env() {
        let parsed = CliInvocation::parse(&args(&["help", "--json"])).unwrap();

        assert!(parsed.json);
        assert_eq!(parsed.command, CodeCommand::Help);
    }

    #[test]
    fn infers_repo_from_workspace_path() {
        assert_eq!(
            infer_repo("/home/dev/workspaces/sulion/branch"),
            Some("sulion".to_string())
        );
    }
}
