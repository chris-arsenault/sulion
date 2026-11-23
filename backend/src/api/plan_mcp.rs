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
use serde_json::json;

use crate::{
    auth::{self, AuthState},
    AppState,
};

mod tools;

const RESOURCE_METADATA: &str = "/.well-known/oauth-protected-resource";
const OAUTH_SCOPES: [&str; 3] = ["openid", "profile", "email"];

struct Connector {
    auth: Arc<AuthState>,
    resource: reqwest::Url,
    cognito_domain: reqwest::Url,
    client_id: String,
    owner: String,
}

pub(super) fn router(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    match Connector::from_env(state) {
        Ok(Some(connector)) => connected_router(state.clone(), Arc::new(connector)),
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
        state.clone(),
        Arc::new(Connector {
            auth,
            resource: "https://sulion.test/mcp".parse().unwrap(),
            cognito_domain: "https://login.sulion.test".parse().unwrap(),
            client_id: "fixture-mcp".to_owned(),
            owner: "fixture-owner".to_owned(),
        }),
    )
    .with_state(state)
}

impl Connector {
    fn from_env(state: &AppState) -> anyhow::Result<Option<Self>> {
        use crate::config::env_optional;

        let Some(client_id) = env_optional("SULION_PLAN_MCP_CLIENT_ID") else {
            return Ok(None);
        };
        let resource = https_url("SULION_PLAN_MCP_URL", "/mcp")?;
        let cognito_domain = https_url("SULION_COGNITO_DOMAIN", "/")?;
        let owner = env_optional("SULION_PLAN_MCP_OWNER")
            .ok_or_else(|| anyhow::anyhow!("SULION_PLAN_MCP_OWNER must name the Cognito owner"))?;
        let auth = state
            .auth
            .clone()
            .ok_or_else(|| anyhow::anyhow!("plan MCP requires configured OAuth validation"))?;
        Ok(Some(Self {
            auth,
            resource,
            cognito_domain,
            client_id,
            owner,
        }))
    }
}

fn https_url(key: &str, path: &str) -> anyhow::Result<reqwest::Url> {
    let value =
        crate::config::env_optional(key).ok_or_else(|| anyhow::anyhow!("{key} must be set"))?;
    let url = if key == "SULION_COGNITO_DOMAIN" && !value.contains("://") {
        reqwest::Url::parse(&format!("https://{value}"))?
    } else {
        reqwest::Url::parse(&value)?
    };
    anyhow::ensure!(
        url.scheme() == "https"
            && url.path() == path
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "invalid HTTPS address for {key}"
    );
    Ok(url)
}

fn connected_router(state: Arc<AppState>, connector: Arc<Connector>) -> Router<Arc<AppState>> {
    let host = connector
        .resource
        .host_str()
        .expect("HTTPS resource has a host");
    let host = match connector.resource.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let browser_url = connector.resource.join("/").expect("same HTTPS origin");
    let service = StreamableHttpService::new(
        move || Ok(tools::PlanTools::new(state.clone(), browser_url.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_max_request_body_bytes(256 * 1024)
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_allowed_hosts([host])
            .enforce_origin_validation(),
    );
    let origin = connector.resource.origin().ascii_serialization();
    let metadata = json!({
        "resource": connector.resource.as_str(),
        "authorization_servers": [origin],
        "bearer_methods_supported": ["header"],
        "scopes_supported": OAUTH_SCOPES,
    });
    let oauth = authorization_metadata(&connector);
    Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(connector, authenticate))
        .route(
            RESOURCE_METADATA,
            get(move || async move { Json(metadata) }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(move || async move { Json(oauth) }),
        )
}

fn authorization_metadata(connector: &Connector) -> serde_json::Value {
    let domain = connector.cognito_domain.as_str().trim_end_matches('/');
    let issuer = connector.auth.issuer_url();
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{domain}/oauth2/authorize"),
        "token_endpoint": format!("{domain}/oauth2/token"),
        "revocation_endpoint": format!("{domain}/oauth2/revoke"),
        "jwks_uri": format!("{issuer}/.well-known/jwks.json"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": OAUTH_SCOPES,
    })
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
    if user.username.as_deref() != Some(connector.owner.as_str()) {
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
    request.extensions_mut().insert(user);
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn challenge(connector: &Connector) -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    let metadata = connector
        .resource
        .join(RESOURCE_METADATA)
        .expect("same HTTPS origin");
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer resource_metadata=\"{metadata}\"")) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}
