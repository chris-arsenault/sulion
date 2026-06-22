use std::ffi::OsString;
use std::path::Path;

use anyhow::{anyhow, Context};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use url::Url;

pub async fn run(args: &[OsString]) -> anyhow::Result<i32> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("retrieve arguments must be valid UTF-8"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(64);
    };
    let env = RetrievalCliEnv::from_env()?;
    let client = reqwest::Client::new();
    match command {
        "search" => search(&client, &env, &args[1..]).await,
        "context" => get_json(&client, &env, "/v1/context", &[], wants_json(&args[1..])).await,
        "sessions" => {
            let options = parse_options(&args[1..])?;
            get_json(
                &client,
                &env,
                "/v1/sessions",
                &options.query_pairs,
                options.json,
            )
            .await
        }
        "facets" => {
            let options = parse_options(&args[1..])?;
            get_json(
                &client,
                &env,
                "/v1/facets",
                &options.query_pairs,
                options.json,
            )
            .await
        }
        "file-history" => file_history(&client, &env, &args[1..]).await,
        "turn" => turn(&client, &env, &args[1..]).await,
        "reindex" => reindex(&client, &env, &args[1..]).await,
        "reset" => reset(&client, &env, &args[1..]).await,
        "index-status" => {
            get_json(
                &client,
                &env,
                "/v1/index/status",
                &[],
                wants_json(&args[1..]),
            )
            .await
        }
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(0)
        }
        other => Err(anyhow!("unknown retrieve command: {other}")),
    }
}

#[derive(Debug, Clone)]
struct RetrievalCliEnv {
    base_url: String,
    token: String,
    repo: Option<String>,
    cwd: Option<String>,
    pty_id: Option<String>,
    workspace_id: Option<String>,
    agent_session_id: Option<String>,
}

impl RetrievalCliEnv {
    fn from_env() -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        Ok(Self {
            base_url: env_required("SULION_RETRIEVAL_URL")?,
            token: env_required("SULION_RETRIEVAL_TOKEN")?,
            repo: env_optional("SULION_REPO_NAME").or_else(|| cwd.as_deref().and_then(infer_repo)),
            cwd,
            pty_id: env_optional("SULION_PTY_ID"),
            workspace_id: env_optional("SULION_WORKSPACE_ID"),
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
                .context("invalid retrieval token header")?,
        );
        insert_header(&mut headers, "x-sulion-repo", self.repo.as_deref())?;
        insert_header(&mut headers, "x-sulion-cwd", self.cwd.as_deref())?;
        insert_header(&mut headers, "x-sulion-pty-id", self.pty_id.as_deref())?;
        insert_header(
            &mut headers,
            "x-sulion-workspace-id",
            self.workspace_id.as_deref(),
        )?;
        insert_header(
            &mut headers,
            "x-sulion-agent-session-id",
            self.agent_session_id.as_deref(),
        )?;
        Ok(headers)
    }

    fn url(&self, path: &str, pairs: &[(&str, String)]) -> anyhow::Result<Url> {
        let mut url = Url::parse(self.base_url.trim_end_matches('/'))
            .context("invalid SULION_RETRIEVAL_URL")?
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

async fn search(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    args: &[String],
) -> anyhow::Result<i32> {
    let options = parse_options(args)?;
    let q = options.positionals.join(" ");
    if q.trim().is_empty() {
        return Err(anyhow!(
            "usage: sulion retrieve search <query> [--include assistant|tools]"
        ));
    }
    let mut pairs = options.query_pairs;
    pairs.push(("q", q));
    let body = request_json(client, env, "GET", "/v1/search", &pairs, None).await?;
    if options.json {
        print_json(&body);
    } else {
        print_search(&body);
    }
    Ok(0)
}

async fn file_history(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    args: &[String],
) -> anyhow::Result<i32> {
    let mut options = parse_options(args)?;
    let Some(path) = options.positionals.first().cloned() else {
        return Err(anyhow!(
            "usage: sulion retrieve file-history <repo/path> [--repo repo]"
        ));
    };
    if !options.query_pairs.iter().any(|(key, _)| *key == "repo") {
        if let Some(repo) = env.repo.clone() {
            options.query_pairs.push(("repo", repo));
        }
    }
    options.query_pairs.push(("path", path));
    let body = request_json(
        client,
        env,
        "GET",
        "/v1/files/history",
        &options.query_pairs,
        None,
    )
    .await?;
    if options.json {
        print_json(&body);
    } else {
        print_file_history(&body);
    }
    Ok(0)
}

async fn turn(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    args: &[String],
) -> anyhow::Result<i32> {
    let mut options = parse_options(args)?;
    if options.positionals.len() != 2 {
        return Err(anyhow!(
            "usage: sulion retrieve turn <agent_session_uuid> <turn_id>"
        ));
    }
    options
        .query_pairs
        .push(("agent_session_uuid", options.positionals[0].clone()));
    options
        .query_pairs
        .push(("turn_id", options.positionals[1].clone()));
    let body = request_json(client, env, "GET", "/v1/turns", &options.query_pairs, None).await?;
    if options.json {
        print_json(&body);
    } else {
        println!("{}", body["markdown"].as_str().unwrap_or_default());
    }
    Ok(0)
}

async fn reindex(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    args: &[String],
) -> anyhow::Result<i32> {
    let options = parse_options(args)?;
    let mut body = serde_json::Map::new();
    for (key, value) in &options.query_pairs {
        match *key {
            "repo" => {
                body.insert("repo".to_string(), json!(value));
            }
            "agent_session_uuid" => {
                body.insert("agent_session_uuid".to_string(), json!(value));
            }
            _ => {}
        }
    }
    if !body.contains_key("repo") {
        if let Some(repo) = env.repo.clone() {
            body.insert("repo".to_string(), json!(repo));
        }
    }
    let response = request_json(
        client,
        env,
        "POST",
        "/v1/reindex",
        &[],
        Some(Value::Object(body)),
    )
    .await?;
    print_json(&response);
    Ok(0)
}

async fn reset(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    args: &[String],
) -> anyhow::Result<i32> {
    let options = parse_options(args)?;
    let mut confirm = false;
    let mut body = serde_json::Map::new();
    for (key, value) in &options.query_pairs {
        match *key {
            "confirm" => confirm = value != "false",
            "reschedule" => {
                body.insert("reschedule".to_string(), json!(value != "false"));
            }
            _ => {}
        }
    }
    if !confirm {
        return Err(anyhow!(
            "refusing to reset the semantic index without --confirm \
             (this drops all embeddings and triggers a full rebuild)"
        ));
    }
    body.insert("confirm".to_string(), json!(true));
    let response = request_json(
        client,
        env,
        "POST",
        "/v1/index/reset",
        &[],
        Some(Value::Object(body)),
    )
    .await?;
    print_json(&response);
    Ok(0)
}

async fn get_json(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    path: &str,
    pairs: &[(&str, String)],
    _json_output: bool,
) -> anyhow::Result<i32> {
    let body = request_json(client, env, "GET", path, pairs, None).await?;
    print_json(&body);
    Ok(0)
}

async fn request_json(
    client: &reqwest::Client,
    env: &RetrievalCliEnv,
    method: &str,
    path: &str,
    pairs: &[(&str, String)],
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let url = env.url(path, pairs)?;
    let request = match method {
        "GET" => client.get(url),
        "POST" => client.post(url),
        _ => return Err(anyhow!("unsupported method: {method}")),
    }
    .headers(env.headers()?);
    let request = if let Some(body) = body {
        request.json(&body)
    } else {
        request
    };
    let response = request.send().await.context("retrieval request failed")?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("retrieval request failed ({status}): {text}"));
    }
    serde_json::from_str(&text).context("retrieval response was not JSON")
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CliOptions {
    json: bool,
    positionals: Vec<String>,
    query_pairs: Vec<(&'static str, String)>,
}

fn parse_options(args: &[String]) -> anyhow::Result<CliOptions> {
    let mut out = CliOptions::default();
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "--json" => out.json = true,
            "--errors-only" => out.query_pairs.push(("errors_only", "true".to_string())),
            "--all" => out.query_pairs.push(("scope", "all".to_string())),
            "--tools" => out.query_pairs.push(("include", "tools".to_string())),
            "--confirm" => out.query_pairs.push(("confirm", "true".to_string())),
            "--no-reschedule" => out.query_pairs.push(("reschedule", "false".to_string())),
            "--repo" => push_value(args, &mut idx, &mut out, "repo")?,
            "--scope" => push_value(args, &mut idx, &mut out, "scope")?,
            "--session" | "--agent-session" | "--agent-session-uuid" => {
                push_value(args, &mut idx, &mut out, "agent_session_uuid")?
            }
            "--include" => push_value(args, &mut idx, &mut out, "include")?,
            "--mode" | "--search-mode" => push_value(args, &mut idx, &mut out, "search_mode")?,
            "--limit" => push_value(args, &mut idx, &mut out, "limit")?,
            "--file" | "--path" => push_value(args, &mut idx, &mut out, "file_path")?,
            "--tool-category" => push_value(args, &mut idx, &mut out, "tool_category")?,
            "--tool-name" => push_value(args, &mut idx, &mut out, "tool_name")?,
            "--agent" => push_value(args, &mut idx, &mut out, "agent")?,
            "--model" => push_value(args, &mut idx, &mut out, "model")?,
            "--since" => push_value(args, &mut idx, &mut out, "since")?,
            "--until" => push_value(args, &mut idx, &mut out, "until")?,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            value if value.starts_with("--") => return Err(anyhow!("unknown option: {value}")),
            value => out.positionals.push(value.to_string()),
        }
        idx += 1;
    }
    Ok(out)
}

fn push_value(
    args: &[String],
    idx: &mut usize,
    options: &mut CliOptions,
    key: &'static str,
) -> anyhow::Result<()> {
    *idx += 1;
    let value = args
        .get(*idx)
        .ok_or_else(|| anyhow!("--{key} requires a value"))?;
    options.query_pairs.push((key, value.clone()));
    Ok(())
}

fn wants_json(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--json")
}

fn print_search(body: &Value) {
    if let Some(warnings) = body["warnings"].as_array() {
        for warning in warnings.iter().filter_map(Value::as_str) {
            eprintln!("warning: {warning}");
        }
    }
    let Some(results) = body["results"].as_array() else {
        print_json(body);
        return;
    };
    for (idx, result) in results.iter().enumerate() {
        let source = result["source_kind"].as_str().unwrap_or("unknown");
        let score = result["score"].as_f64().unwrap_or_default();
        let repo = result["repo"].as_str().unwrap_or("-");
        let session = result["agent_session_uuid"].as_str().unwrap_or("-");
        let turn = result["turn_id"]
            .as_i64()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{}. {source} score={score:.3} repo={repo} session={session} turn={turn}",
            idx + 1
        );
        if let Some(tool) = result["tool"].as_object() {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("-");
            let category = tool
                .get("operation_category")
                .and_then(Value::as_str)
                .unwrap_or("-");
            println!("   tool={name} category={category}");
        }
        if let Some(snippet) = result["snippet"].as_str() {
            println!("   {}", snippet);
        }
        if let Some(files) = result["evidence"]["file_touches"].as_array() {
            for file in files.iter().take(4) {
                if let Some(path) = file["path"].as_str() {
                    println!("   file: {path}");
                }
            }
        }
    }
}

fn print_file_history(body: &Value) {
    let Some(items) = body.as_array() else {
        print_json(body);
        return;
    };
    for item in items {
        let timestamp = item["timestamp"].as_str().unwrap_or("-");
        let session = item["agent_session_uuid"].as_str().unwrap_or("-");
        let turn = item["turn_id"].as_i64().unwrap_or_default();
        let kind = item["touch_kind"].as_str().unwrap_or("-");
        let preview = item["preview"].as_str().unwrap_or_default();
        println!("{timestamp} session={session} turn={turn} {kind} {preview}");
    }
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    );
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

fn print_usage() {
    eprintln!(
        "usage:
  sulion retrieve search <query> [--mode hybrid|lexical|semantic] [--include assistant|tools] [--repo repo] [--limit n]
  sulion retrieve file-history <repo/path> [--repo repo]
  sulion retrieve turn <agent_session_uuid> <turn_id>
  sulion retrieve sessions [--repo repo]
  sulion retrieve facets [--repo repo]
  sulion retrieve index-status
  sulion retrieve reindex [--repo repo]
  sulion retrieve reset --confirm [--no-reschedule]

options: --json --scope repo|session|all --session uuid --tool-category category --tool-name name --file path --errors-only"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_options_and_positionals() {
        let args = vec![
            "retrieval".to_string(),
            "api".to_string(),
            "--include".to_string(),
            "tools".to_string(),
            "--limit".to_string(),
            "3".to_string(),
            "--json".to_string(),
        ];
        let parsed = parse_options(&args).unwrap();
        assert!(parsed.json);
        assert_eq!(parsed.positionals, vec!["retrieval", "api"]);
        assert!(parsed
            .query_pairs
            .contains(&("include", "tools".to_string())));
        assert!(parsed.query_pairs.contains(&("limit", "3".to_string())));
    }

    #[test]
    fn infers_repo_from_workspace_path() {
        assert_eq!(
            infer_repo("/home/dev/workspaces/sulion/abc"),
            Some("sulion".to_string())
        );
    }
}
