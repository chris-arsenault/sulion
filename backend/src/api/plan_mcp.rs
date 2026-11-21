use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    auth::{self, AuthState},
    AppState,
};

mod tools;

#[derive(Clone)]
struct Grant {
    subject: String,
    repos: Vec<String>,
}

struct Connector {
    auth: Arc<AuthState>,
    resource: reqwest::Url,
    client_id: String,
    grant: Grant,
}

#[derive(Deserialize)]
struct ConnectorSettings {
    resource: String,
    client_id: String,
    subject: String,
    repos: Vec<String>,
}

pub(super) fn router(state: &AppState) -> Router<Arc<AppState>> {
    match Connector::from_env(state) {
        Ok(Some(connector)) => connected_router(state.pool.clone(), Arc::new(connector)),
        Ok(None) => Router::new(),
        Err(error) => {
            tracing::error!(%error, "plan MCP endpoint disabled");
            Router::new()
        }
    }
}

#[cfg(feature = "integration-tests")]
pub fn test_plan_mcp_router(state: Arc<AppState>, auth: Arc<AuthState>) -> Router {
    connected_router(
        state.pool.clone(),
        Arc::new(Connector {
            auth,
            resource: "https://sulion.test/mcp".parse().unwrap(),
            client_id: "fixture-mcp".to_owned(),
            grant: Grant {
                subject: "fixture-user".to_owned(),
                repos: vec!["alpha".to_owned()],
            },
        }),
    )
    .with_state(state)
}

impl Connector {
    fn from_env(state: &AppState) -> anyhow::Result<Option<Self>> {
        let Some(value) = crate::config::env_optional("SULION_PLAN_MCP_CONFIG") else {
            return Ok(None);
        };
        let Some(ConnectorSettings {
            resource,
            client_id,
            subject,
            repos,
        }) = serde_json::from_str::<Option<ConnectorSettings>>(&value)?
        else {
            return Ok(None);
        };
        let resource = reqwest::Url::parse(&resource)?;
        anyhow::ensure!(
            resource.scheme() == "https"
                && resource.path() == "/mcp"
                && resource.query().is_none()
                && resource.fragment().is_none()
                && resource.username().is_empty()
                && resource.password().is_none(),
            "plan MCP resource must be the connector's HTTPS /mcp URL"
        );
        anyhow::ensure!(
            !client_id.trim().is_empty() && !subject.trim().is_empty() && !repos.is_empty(),
            "plan MCP requires a client ID, subject and connected repositories"
        );
        for repo in &repos {
            super::routes::validate_repo_name(repo)
                .map_err(|_| anyhow::anyhow!("invalid repository in SULION_PLAN_MCP_REPOS"))?;
        }
        let auth = state
            .auth
            .clone()
            .ok_or_else(|| anyhow::anyhow!("plan MCP requires configured OAuth validation"))?;
        Ok(Some(Self {
            auth,
            resource,
            client_id,
            grant: Grant { subject, repos },
        }))
    }
}

fn connected_router(pool: crate::db::Pool, connector: Arc<Connector>) -> Router<Arc<AppState>> {
    let host = connector
        .resource
        .host_str()
        .expect("HTTPS resource has a host");
    let host = match connector.resource.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let service = StreamableHttpService::new(
        move || Ok(tools::PlanTools::new(pool.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_allowed_hosts([host])
            .enforce_origin_validation(),
    );
    let metadata = json!({"resource": connector.resource.as_str(), "authorization_servers": [connector.auth.issuer_url()]});
    Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(connector, authenticate))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(move || async move { Json(metadata) }),
        )
}

async fn authenticate(
    State(connector): State<Arc<Connector>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token else {
        return challenge(&connector);
    };
    let user = match connector
        .auth
        .validate_plan_connector(token, &connector.client_id, connector.resource.as_str())
        .await
    {
        Ok(user) => user,
        Err(error) => {
            let response = auth::authentication_error_response(&error);
            return if response.status() == StatusCode::UNAUTHORIZED {
                challenge(&connector)
            } else {
                response
            };
        }
    };
    if user.sub != connector.grant.subject {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(error) = user.check_authority().await {
        let response = auth::authentication_error_response(&error);
        return if response.status() == StatusCode::UNAUTHORIZED {
            challenge(&connector)
        } else {
            response
        };
    }
    request.extensions_mut().insert(connector.grant.clone());
    next.run(request).await
}

fn challenge(connector: &Connector) -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    let metadata = connector
        .resource
        .join("/.well-known/oauth-protected-resource/mcp")
        .expect("same HTTPS origin");
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer resource_metadata=\"{metadata}\"")) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}
