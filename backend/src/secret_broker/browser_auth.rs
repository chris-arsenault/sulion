use super::*;

pub(super) async fn check_browser_authority(
    State(state): State<Arc<BrokerState>>,
    Json(authority): Json<crate::auth::revocation::Authority>,
) -> StatusCode {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        crate::auth::revocation::allowed(&state.pool, &authority),
    )
    .await
    {
        Ok(Ok(true)) => StatusCode::NO_CONTENT,
        _ => StatusCode::UNAUTHORIZED,
    }
}

pub(super) async fn revoke_browser_session(
    State(state): State<Arc<BrokerState>>,
    Extension(user): Extension<AuthenticatedUser>,
) -> StatusCode {
    match crate::auth::revocation::revoke(&state.pool, &user.sub).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[derive(Deserialize)]
pub(super) struct RevokePrincipal {
    sub: String,
}

pub(super) async fn revoke_browser_principal(
    State(state): State<Arc<BrokerState>>,
    Json(principal): Json<RevokePrincipal>,
) -> StatusCode {
    match crate::auth::revocation::revoke(&state.pool, &principal.sub).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
