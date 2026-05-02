use sulion::ingest::{Ingester, IngesterConfig};
use sulion::{app, config::Config, db, AppState};
use tracing_subscriber::EnvFilter;

const INGESTER_RESTART_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if argv
        .get(1)
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "agent-launcher")
    {
        let cfg = sulion::agent::parse_launcher_args(&argv[2..])?;
        let code = sulion::agent::run_launcher(cfg).await?;
        std::process::exit(code);
    }
    if argv
        .get(1)
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "codex-launcher")
    {
        let cfg = sulion::codex::parse_launcher_args(&argv[2..])?;
        let code = sulion::codex::run_launcher(cfg).await?;
        std::process::exit(code);
    }
    if argv
        .get(1)
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "credential-helper")
    {
        let code = sulion::credential_helper::run(&argv[2..]).await?;
        std::process::exit(code);
    }

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(listen = %cfg.listen, "sulion starting");

    let auth = cfg
        .auth
        .clone()
        .map(sulion::auth::AuthState::new)
        .map(std::sync::Arc::new);
    if let Some(auth_cfg) = cfg.auth.as_ref() {
        tracing::info!(issuer = %auth_cfg.issuer_url, client_id = %auth_cfg.client_id, "jwt auth enabled");
    } else {
        tracing::warn!("jwt auth disabled; SULION_AUTH_ISSUER_URL not set");
    }

    let pool = db::connect(&cfg.db_url).await?;
    db::run_migrations(&pool).await?;
    tracing::info!("migrations applied");

    // Any row still marked 'live' belongs to a shell the prior backend
    // was supervising — the process died with its parent and nobody
    // captured exit. Roll those to 'orphaned'.
    let orphaned = sulion::pty::reconcile_orphans_on_startup(&pool).await?;
    if orphaned > 0 {
        tracing::info!(count = orphaned, "reconciled orphaned PTY sessions");
    }

    // One-shot backfill: synthesize canonical event_blocks for any
    // events ingested before the canonical-block migration. No-op once
    // complete. Cheap — corpus is small and the SELECT anti-joins are
    // indexed.
    match sulion::ingest::backfill_canonical_blocks(&pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(count = n, "backfilled canonical event blocks"),
        Err(err) => tracing::warn!(%err, "canonical block backfill failed"),
    }

    match sulion::ingest::backfill_timeline_projection(&pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(sessions = n, "backfilled app timeline projection"),
        Err(err) => tracing::warn!(%err, "timeline projection backfill failed"),
    }

    // Background ingester — the sole reader of the JSONL transcripts.
    // We hold an Arc so the `/api/stats` handler can read its runtime
    // totals without a second process observing them.
    let ingester = std::sync::Arc::new(Ingester::new());
    let ingester_pool = pool.clone();
    let ingester_cfg = IngesterConfig::new(cfg.claude_projects_dir.clone())
        .with_codex_sessions_dir(cfg.codex_sessions_dir.clone());
    let ingester_supervisor = ingester.clone();
    tokio::spawn(async move {
        run_ingester_supervisor(ingester_supervisor, ingester_pool, ingester_cfg).await;
    });
    tracing::info!(
        claude_projects = %cfg.claude_projects_dir.display(),
        codex_sessions = %cfg.codex_sessions_dir.display(),
        "ingester started",
    );

    // SessionStart-hook correlation socket.
    let correlate_pool = pool.clone();
    let correlate_sock = cfg.correlate_sock_path.clone();
    tokio::spawn(async move {
        if let Err(err) = sulion::correlate::run(correlate_pool, correlate_sock).await {
            tracing::error!(%err, "correlate socket exited");
        }
    });

    let state = AppState::new_with_auth(
        pool,
        cfg.repos_root.clone(),
        cfg.library_root.clone(),
        ingester,
        auth,
    );
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}

async fn run_ingester_supervisor(
    ingester: std::sync::Arc<Ingester>,
    pool: db::Pool,
    cfg: IngesterConfig,
) {
    loop {
        let ingester_run = ingester.clone();
        let pool_run = pool.clone();
        let cfg_run = cfg.clone();
        let handle = tokio::spawn(async move {
            ingester_run.run(pool_run, cfg_run).await;
        });

        match handle.await {
            Ok(()) => {
                tracing::error!("ingester task exited unexpectedly; restarting");
            }
            Err(err) if err.is_panic() => {
                tracing::error!(%err, "ingester task panicked; restarting");
            }
            Err(err) => {
                tracing::error!(%err, "ingester task aborted; restarting");
            }
        }

        tokio::time::sleep(INGESTER_RESTART_BACKOFF).await;
    }
}
