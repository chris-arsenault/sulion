use std::ffi::OsString;

pub async fn run_cli(args: &[OsString]) -> anyhow::Result<i32> {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("status") => {
            print_workspace_status();
            Ok(0)
        }
        _ => {
            eprintln!("usage: sulion workspace status");
            Ok(2)
        }
    }
}

fn print_workspace_status() {
    let keys = [
        "SULION_REPO_NAME",
        "SULION_WORKSPACE_ID",
        "SULION_WORKSPACE_KIND",
        "SULION_WORKSPACE_PATH",
        "SULION_CANONICAL_REPO",
        "SULION_BRANCH",
        "SULION_BASE_REF",
        "SULION_BASE_SHA",
        "SULION_MERGE_TARGET",
    ];
    if std::env::var("SULION_WORKSPACE_ID").is_err() {
        println!("No Sulion workspace metadata is present in this shell.");
        return;
    }
    println!("Sulion workspace");
    for key in keys {
        let value = std::env::var(key).unwrap_or_default();
        println!("{key}={value}");
    }
}
