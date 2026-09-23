//! `sulion archive`: queue archive cycles, verifies, and restores for the
//! control process's loop, and read its state. Same transport as `plan`:
//! a typed request over the correlate socket.

use std::ffi::OsString;

use anyhow::{anyhow, bail};
use serde_json::Value;
use uuid::Uuid;

use crate::correlate::ControlRequest;

use super::usage::print_archive_usage;
use super::{
    reject_unknown_options, run_control, take_flag, take_option, usage_failure, utf8_args,
    ResponseKind,
};

pub async fn run_archive(args: &[OsString]) -> anyhow::Result<i32> {
    let mut args = utf8_args(args, "archive")?;
    let json = take_flag(&mut args, "--json");
    let Some(command) = args.first().cloned() else {
        print_archive_usage();
        return Ok(64);
    };
    if matches!(command.as_str(), "help" | "-h" | "--help") {
        print_archive_usage();
        return Ok(0);
    }
    args.remove(0);
    let request = match parse_archive_request(&command, &mut args) {
        Ok(request) => request,
        Err(err) => return Ok(usage_failure(ResponseKind::Archive, &err)),
    };
    run_control(request, json, ResponseKind::Archive).await
}

fn parse_archive_request(command: &str, args: &mut Vec<String>) -> anyhow::Result<ControlRequest> {
    let request = match command {
        "run" => {
            let dry_run = take_flag(args, "--dry-run");
            reject_unknown_options(args)?;
            ControlRequest::ArchiveRun { dry_run }
        }
        "restore" => ControlRequest::ArchiveRestore {
            scope: parse_restore_scope(args)?,
        },
        "status" => {
            reject_unknown_options(args)?;
            ControlRequest::ArchiveStatus
        }
        "verify" => {
            let deep = take_flag(args, "--deep");
            reject_unknown_options(args)?;
            ControlRequest::ArchiveVerify { deep }
        }
        "list" => {
            let limit = match take_option(args, "--limit")? {
                Some(raw) => Some(
                    raw.parse::<i64>()
                        .map_err(|_| anyhow!("--limit must be a number"))?,
                ),
                None => None,
            };
            reject_unknown_options(args)?;
            ControlRequest::ArchiveList { limit }
        }
        other => bail!("unknown archive command: {other}"),
    };
    Ok(request)
}

fn parse_restore_scope(args: &mut Vec<String>) -> anyhow::Result<crate::archive::RestoreScope> {
    let session_uuid = match take_option(args, "--session")? {
        Some(raw) => Some(
            raw.parse::<Uuid>()
                .map_err(|_| anyhow!("--session must be an agent session uuid"))?,
        ),
        None => None,
    };
    let month = take_option(args, "--month")?;
    if let Some(month) = month.as_deref() {
        let valid = month.len() == 7
            && month.as_bytes()[4] == b'-'
            && month[..4].chars().all(|c| c.is_ascii_digit())
            && month[5..].chars().all(|c| c.is_ascii_digit());
        if !valid {
            bail!("--month must be YYYY-MM");
        }
    }
    let repo = take_option(args, "--repo")?;
    let all = take_flag(args, "--all");
    let purge_after = take_flag(args, "--purge-after");
    reject_unknown_options(args)?;
    let scope = crate::archive::RestoreScope {
        session_uuid,
        month,
        repo,
        all,
        purge_after,
    };
    if scope.is_empty() {
        bail!("restore needs --session, --month, --repo, or --all");
    }
    Ok(scope)
}

pub(super) fn print_archive_data(data: &Value) {
    if let Some(items) = data.as_array() {
        if items.is_empty() {
            println!("No archive requests.");
            return;
        }
        for item in items {
            print_archive_request(item);
        }
        return;
    }
    if data.get("configured").is_some() {
        print_archive_status(data);
        return;
    }
    if data.get("kind").is_some() {
        print_archive_request(data);
        println!("next: sulion archive list");
        return;
    }
    println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
}

fn print_archive_status(data: &Value) {
    let configured = data["configured"].as_bool().unwrap_or(false);
    println!(
        "store: {}",
        if configured {
            data["store"].as_str().unwrap_or("(configured)")
        } else {
            "not configured (SULION_ARCHIVE_BUCKET unset)"
        }
    );
    println!(
        "purging: {}",
        if data["purge_enabled"].as_bool().unwrap_or(false) {
            format!(
                "enabled since {} (SULION_ARCHIVE_PURGE_ENABLED in compose.yaml)",
                data["purge_enabled_at"].as_str().unwrap_or("?")
            )
        } else {
            "disabled — cycles export and dump only; enable it with a commit to \
             SULION_ARCHIVE_PURGE_ENABLED in compose.yaml"
                .to_string()
        }
    );
    println!(
        "last cycle: started {} completed {}",
        data["last_cycle_started_at"].as_str().unwrap_or("never"),
        data["last_cycle_completed_at"].as_str().unwrap_or("never"),
    );
    println!(
        "last dump: {} at {}",
        data["last_dump_key"].as_str().unwrap_or("none"),
        data["last_dump_at"].as_str().unwrap_or("never"),
    );
    println!(
        "sessions: {} archived, {} purged, {} archived bytes",
        data["sessions_archived"].as_i64().unwrap_or(0),
        data["sessions_purged"].as_i64().unwrap_or(0),
        data["archived_bytes"].as_i64().unwrap_or(0),
    );
    println!(
        "requests: {} pending or running",
        data["pending_requests"].as_i64().unwrap_or(0)
    );
    if let Some(recent) = data["recent_requests"].as_array() {
        for item in recent.iter().take(10) {
            print_archive_request(item);
        }
    }
}

fn print_archive_request(item: &Value) {
    let detail = item["error"]
        .as_str()
        .map(|error| format!(" — {error}"))
        .unwrap_or_default();
    println!(
        "#{} {} [{}] requested {} {}{}",
        item["id"].as_i64().unwrap_or(0),
        item["kind"].as_str().unwrap_or("?"),
        item["status"].as_str().unwrap_or("?"),
        item["requested_at"].as_str().unwrap_or(""),
        item["scope"]
            .as_object()
            .filter(|scope| !scope.is_empty())
            .map(|scope| serde_json::to_string(scope).unwrap_or_default())
            .unwrap_or_default(),
        detail,
    );
}
