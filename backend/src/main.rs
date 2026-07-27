use sulion::ingest::{Ingester, IngesterConfig};
use sulion::{app, config::Config, db, AppState};
use tracing_subscriber::EnvFilter;

const INGESTER_RESTART_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(code) = dispatch_cli(&argv).await? {
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

    // Legacy in-process live rows died with their backend. Node-owned rows
    // are deliberately left live until the owning node reports its boot
    // inventory, so a control-only restart cannot kill durable sessions.
    let orphaned = sulion::pty::reconcile_orphans_on_startup(&pool).await?;
    if orphaned > 0 {
        tracing::info!(count = orphaned, "reconciled orphaned PTY sessions");
    }

    let ingester = std::sync::Arc::new(Ingester::new());
    let ingester_cfg = IngesterConfig::new(cfg.claude_projects_dir.clone())
        .with_codex_sessions_dir(cfg.codex_sessions_dir.clone());

    let state = AppState::new_with_auth_and_node_mode(
        pool.clone(),
        cfg.repos_root.clone(),
        cfg.workspaces_root.clone(),
        cfg.library_root.clone(),
        ingester.clone(),
        auth,
        true,
    );
    if let Some(node) = cfg.standalone_node.as_ref() {
        let runtime = sulion::node_runtime::NodeRuntime::new(
            node.node_id,
            uuid::Uuid::new_v4(),
            pool.clone(),
            cfg.repos_root.clone(),
            cfg.workspaces_root.clone(),
        );
        let boot_id = state
            .node_control
            .start_runtime_loopback(runtime.clone(), &node.display_name)
            .await?;
        runtime.run_background_managers().await;
        tracing::info!(
            node_id = %node.node_id,
            %boot_id,
            "standalone node loopback connected"
        );
    }

    let maintenance_pool = pool.clone();
    tokio::spawn(async move {
        run_control_maintenance(maintenance_pool).await;
    });

    if cfg.standalone_node.is_some() {
        let correlate_pool = pool.clone();
        let correlate_sock = cfg.correlate_sock_path.clone();
        tokio::spawn(async move {
            if let Err(err) = sulion::correlate::run(correlate_pool, correlate_sock).await {
                tracing::error!(%err, "correlate socket exited");
            }
        });
        tokio::spawn(run_ingester_supervisor(
            ingester.clone(),
            pool.clone(),
            ingester_cfg,
        ));
    }
    tokio::spawn(sulion::api::run_stats_sampler(state.clone()));
    tokio::spawn(sulion::ingest::run_usage_backfill(pool.clone()));
    tokio::spawn(state.node_control.clone().run_heartbeat_monitor());

    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!(listen = %cfg.listen, "api listener bound");
    axum::serve(listener, app(state)).await?;
    Ok(())
}

async fn dispatch_cli(argv: &[std::ffi::OsString]) -> anyhow::Result<Option<i32>> {
    let Some(command) = argv.get(1).and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let args = &argv[2..];
    let code = match command {
        "agent-launcher" => {
            let cfg = sulion::agent::parse_launcher_args(args)?;
            sulion::agent::run_launcher(cfg).await?
        }
        "codex-launcher" => {
            let cfg = sulion::codex::parse_launcher_args(args)?;
            sulion::codex::run_launcher(cfg).await?
        }
        "credential-helper" => sulion::credential_helper::run(args).await?,
        "runner-client" => sulion::container_runner::run_client(args).await?,
        "postgres" => sulion::container_runner::run_postgres_cli(args).await?,
        "workspace" => sulion::worktree::run_cli(args).await?,
        "retrieve" => sulion::retrieval_cli::run(args).await?,
        "code" => sulion::code_cli::run(args).await?,
        "plan" => sulion::plan_cli::run_plan(args).await?,
        "activity" => sulion::plan_cli::run_activity(args).await?,
        "name" => sulion::plan_cli::run_name(args).await?,
        _ => return Ok(None),
    };
    Ok(Some(code))
}

async fn run_control_maintenance(pool: db::Pool) {
    match sulion::ingest::run_required_startup_maintenance(&pool).await {
        Ok(stats) => {
            tracing::info!(
                canonical_events_backfilled = stats.canonical_events_backfilled,
                timeline_sessions_backfilled = stats.timeline_sessions_backfilled,
                "startup transcript maintenance complete",
            );
        }
        Err(err) => {
            tracing::warn!(%err, "startup transcript maintenance failed");
        }
    }
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
