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

    let ingester = std::sync::Arc::new(Ingester::new());
    let ingester_cfg = IngesterConfig::new(cfg.claude_projects_dir.clone())
        .with_codex_sessions_dir(cfg.codex_sessions_dir.clone());

    let state = AppState::new_with_auth(
        pool.clone(),
        cfg.repos_root.clone(),
        cfg.workspaces_root.clone(),
        cfg.library_root.clone(),
        ingester.clone(),
        auth,
    );
    let node_tls = apply_node_policy(&state).await?;
    if let Some(node) = cfg.standalone_node.as_ref() {
        start_standalone_node(&state, &cfg, pool.clone(), node).await?;
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
    tokio::spawn(state.node_control.clone().run_heartbeat_monitor());

    // The published LAN port for nodes serves TLS terminated in this process
    // — no kernel tunnel, no elevated privileges anywhere — and carries only
    // the node router plus the broker/retrieval gateway, so the rest of the
    // API is never LAN-exposed. In-process rather than a proxy hop so the LAN
    // source check sees each node's real address. The certificate is
    // self-generated, pinned by nodes on first pairing, and its digest is
    // signed into the handshake proof.
    if let (Ok(node_listen), Some(tls)) = (std::env::var("SULION_NODE_LISTEN"), node_tls) {
        let node_listen: std::net::SocketAddr = node_listen
            .parse()
            .map_err(|_| anyhow::anyhow!("SULION_NODE_LISTEN must be host:port"))?;
        let node_app = axum::Router::new()
            .merge(sulion::node_protocol::public_router())
            .merge(sulion::node_protocol::gateway::router())
            .with_state(state.clone());
        let rustls_config = axum_server::tls_rustls::RustlsConfig::from_config(
            std::sync::Arc::new(tls.server_config()?),
        );
        tracing::info!(listen = %node_listen, "node TLS listener bound");
        tokio::spawn(async move {
            if let Err(error) = axum_server::bind_rustls(node_listen, rustls_config)
                .serve(node_app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
            {
                tracing::error!(%error, "node listener exited");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!(listen = %cfg.listen, "api listener bound");
    // ConnectInfo is what the node LAN guard reads, so the node socket must be
    // served through a make-service that carries the peer address.
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Applies the node admission boundary, the runtime configuration this control
/// plane hands to authenticated nodes, and the TLS identity the node port
/// serves. Returns the TLS material for the listener when one is configured.
/// The loopback role hosts its devenv as a child process: same PTY
/// implementation and wire protocol as the dedicated node, no survival
/// across backend restarts (requirement 4 of the plan).
async fn start_standalone_node(
    state: &std::sync::Arc<sulion::AppState>,
    cfg: &sulion::config::Config,
    pool: db::Pool,
    node: &sulion::config::StandaloneNodeConfig,
) -> anyhow::Result<()> {
    let (devenv_link, devenv_events) = sulion::devenv::link::DevenvLink::new();
    sulion::devenv::launcher::start(
        devenv_link.clone(),
        sulion::devenv::launcher::LauncherConfig::from_env(),
    );
    let devenv_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    while !devenv_link.connected().await {
        if tokio::time::Instant::now() >= devenv_deadline {
            tracing::warn!("devenv did not connect in time; PTY spawns will fail until it does");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let runtime = sulion::node_runtime::NodeRuntime::new(
        node.node_id,
        uuid::Uuid::new_v4(),
        pool,
        cfg.repos_root.clone(),
        cfg.workspaces_root.clone(),
        devenv_link,
        devenv_events,
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
    Ok(())
}

async fn apply_node_policy(
    state: &AppState,
) -> anyhow::Result<Option<sulion::node_protocol::tls::ControlTls>> {
    let source_policy = sulion::node_protocol::NodeSourcePolicy::from_env()?;
    if source_policy.is_unrestricted() {
        tracing::warn!("node LAN boundary disabled; SULION_NODE_LAN_CIDR is empty");
    }
    let delivered_config = sulion::node_protocol::NodeRuntimeConfig::from_env()?;
    match delivered_config.as_ref() {
        Some(config) => tracing::info!(
            digest = %config.digest(),
            keys = ?config.key_names(),
            "node runtime configuration ready for delivery",
        ),
        None => tracing::warn!(
            "no node runtime configuration to deliver; approved nodes must be \
             configured out of band"
        ),
    }
    // A node pins this identity the first time it pairs and refuses any later
    // peer that cannot sign for it.
    let identity = sulion::node_protocol::ControlIdentity::load_or_create(&state.pool).await?;
    tracing::info!(control_key = %identity.public_key(), "control plane identity ready");

    // TLS for the node port, generated in-process: encryption without any
    // elevated privilege. The digest rides the signed handshake proof so the
    // certificate a node pins is bound to the identity above.
    let node_tls = if std::env::var("SULION_NODE_LISTEN").is_ok() {
        let sans = std::env::var("SULION_NODE_TLS_SAN").unwrap_or_else(|_| "192.168.66.3".into());
        let tls =
            sulion::node_protocol::tls::ControlTls::load_or_create(&state.pool, &sans).await?;
        tracing::info!(digest = %tls.digest, %sans, "control TLS identity ready");
        Some(tls)
    } else {
        None
    };

    state.node_control.apply_policy(
        delivered_config,
        source_policy,
        Some(identity),
        node_tls.as_ref().map(|tls| tls.digest.clone()),
    )?;
    Ok(node_tls)
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
                usage_sessions_backfilled = stats.usage_sessions_backfilled,
                "startup transcript maintenance complete",
            );
        }
        Err(err) => {
            // Alternate format prints the whole context chain — the top
            // context alone ("backfill canonical blocks") says nothing
            // about which row or database error was responsible.
            tracing::warn!(
                error = format!("{err:#}"),
                "startup transcript maintenance failed"
            );
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
