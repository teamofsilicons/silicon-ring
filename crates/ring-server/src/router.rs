//! Routes, CORS, and the small public endpoints (`/healthz`, `/v1/meta`, account lookup).

use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware;
use axum::routing::{delete, get, patch, post, put};
use ring_proto::{AccountLookup, Health, Limits, Meta, RedirectUris};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::app::AppState;
use crate::calls::api as calls;
use crate::error::ApiResult;
use crate::extract::Auth;
use crate::{
    hooks, idempotency, media, realtime, sessions, settings, ting, transcript, voicemail, web,
};

pub fn build(app: AppState) -> Router {
    let idempotent = || middleware::from_fn_with_state(app.clone(), idempotency::layer);
    let api = Router::new()
        .route("/healthz", get(health))
        .route("/v1/meta", get(meta))
        .route("/v1/auth/silicon", post(sessions::silicon_login))
        .route("/v1/auth/carbon", post(sessions::carbon_login))
        .route("/v1/auth/web/start", get(sessions::web_start))
        .route("/v1/auth/web/callback", get(sessions::web_callback))
        .route("/v1/me", get(sessions::me))
        .route("/v1/session", delete(sessions::logout))
        .route("/v1/devices/current", put(sessions::update_current))
        .route("/v1/devices", get(sessions::list_devices))
        .route(
            "/v1/devices/{device_id}",
            patch(sessions::patch_device).delete(sessions::delete_device),
        )
        .route("/v1/accounts/{reference}", get(account_lookup))
        .route("/v1/people/recent", get(calls::recent_people))
        .route(
            "/v1/calls",
            get(calls::list).merge(post(calls::create).route_layer(idempotent())),
        )
        .route("/v1/calls/{ringid}", get(calls::get))
        .route("/v1/calls/{ringid}/accept", post(calls::accept))
        .route("/v1/calls/{ringid}/decline", post(calls::decline))
        .route("/v1/calls/{ringid}/silence", post(calls::silence))
        .route("/v1/calls/{ringid}/leave", post(calls::leave))
        .route(
            "/v1/calls/{ringid}/invite",
            post(calls::invite).route_layer(idempotent()),
        )
        .route("/v1/calls/{ringid}/media", post(calls::media))
        .route(
            "/v1/calls/{ringid}/transcript",
            get(transcript::get_transcript),
        )
        .route("/v1/calls/{ringid}/recording", get(calls::recording))
        .route(
            "/v1/calls/{ringid}/messages",
            post(transcript::send_message).route_layer(idempotent()),
        )
        .route(
            "/v1/calls/{ringid}/delegations",
            get(transcript::list_delegations),
        )
        .route(
            "/v1/calls/{ringid}/voicemail",
            post(calls::leave_voicemail).route_layer(idempotent()),
        )
        .route(
            "/v1/delegations/{delegation_id}",
            get(transcript::get_delegation),
        )
        .route("/v1/voicemails", get(voicemail::list))
        .route(
            "/v1/voicemails/{id}",
            get(voicemail::get)
                .patch(voicemail::patch)
                .delete(voicemail::delete),
        )
        .route(
            "/v1/settings",
            get(settings::get_settings).patch(settings::patch_settings),
        )
        .route("/v1/voices", get(settings::voices))
        .route("/v1/context/preview", post(settings::preview))
        .route(
            "/v1/context/approvals",
            get(settings::list_approvals).post(settings::approve),
        )
        .route(
            "/v1/context/approvals/{id}",
            delete(settings::revoke_approval),
        )
        .route(
            "/v1/notifications",
            get(ting::get_notifications).post(ting::set_notifications),
        )
        .route("/v1/ws", get(realtime::ws))
        .route("/v1/accounts/webhook", post(hooks::accounts_webhook))
        .route("/internal/media", get(media::internal_media))
        .route("/internal/livekit/webhook", post(hooks::livekit_webhook))
        .fallback(web::serve)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(cors(&app))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ));
    api.with_state(app)
}

fn cors(app: &AppState) -> CorsLayer {
    let origins: Vec<HeaderValue> = app
        .config
        .allowed_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-ring-client"),
            HeaderName::from_static("idempotency-key"),
        ])
        .max_age(Duration::from_secs(600))
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        app: "ring".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

async fn meta(State(app): State<AppState>) -> Json<Meta> {
    Json(Meta {
        app_id: app.config.accounts_app_id.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: ring_proto::API_VERSION.into(),
        accounts_url: app.config.accounts_url.clone(),
        web_url: app.config.web_url.clone(),
        redirect_uris: RedirectUris {
            native: sessions::NATIVE_REDIRECT_URI.into(),
            loopback: sessions::LOOPBACK_REDIRECT_URI.into(),
        },
        limits: Limits::default(),
        ringing_seconds: ring_proto::RINGING_SECONDS,
        voicemail_max_seconds: ring_proto::VOICEMAIL_MAX_SECONDS,
        voices: ring_proto::natural_voices(),
        default_voice: ring_proto::DEFAULT_VOICE.into(),
        webpush_public_key: app.config.vapid.as_ref().map(|v| v.public_key.clone()),
    })
}

async fn account_lookup(
    State(app): State<AppState>,
    _auth: Auth,
    Path(reference): Path<String>,
) -> ApiResult<Json<AccountLookup>> {
    let account = crate::accounts::resolve(&app, &reference).await?;
    let on_ring = app
        .db
        .with(|c| crate::accounts::get_row(c, &account.uuid))?
        .is_some_and(|r| r.on_ring);
    Ok(Json(AccountLookup { account, on_ring }))
}
