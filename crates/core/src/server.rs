//! Local HTTP API for the browser extension and the phone web app.
//!
//! Listens on 127.0.0.1 only. The phone reaches it through `tailscale serve`
//! (HTTPS, tailnet-only). Every `/api` call needs the pairing token, except
//! `/api/pair-local`, which hands the token to callers on this Mac only.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::cards::{self, Card, SyncRequest};
use crate::{audio, dict};
use crate::explain::{self, ExplainRequest, LlmConfig};
use crate::service::{AddCardRequest, Service};

pub const DEFAULT_PORT: u16 = 7331;

type AppState = Arc<Service>;

struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{:#}", self.0) }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// The pairing token, created on first use.
pub fn token(svc: &Service) -> Result<String> {
    if let Some(t) = svc.db.get_setting("auth.token")? {
        return Ok(t);
    }
    use rand::Rng;
    let bytes: [u8; 24] = rand::rng().random();
    let t = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    svc.db.set_setting("auth.token", Some(&t))?;
    Ok(t)
}

/// A request from this Mac itself (not proxied by tailscale serve, not a web page).
fn is_local_client(addr: &SocketAddr, headers: &HeaderMap) -> bool {
    if !addr.ip().is_loopback() {
        return false;
    }
    let proxied = ["x-forwarded-for", "forwarded", "tailscale-user-login", "x-forwarded-host"]
        .iter()
        .any(|h| headers.contains_key(*h));
    if proxied {
        return false;
    }
    match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        None => true,
        Some(o) => {
            o.starts_with("chrome-extension://")
                || o.starts_with("moz-extension://")
                || o.starts_with("safari-web-extension://")
                || o == format!("http://127.0.0.1:{DEFAULT_PORT}")
                || o == format!("http://localhost:{DEFAULT_PORT}")
        }
    }
}

async fn auth(State(svc): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/") || path == "/api/pair-local" {
        return next.run(req).await;
    }
    let expected = match token(&svc) {
        Ok(t) => t,
        Err(e) => return ApiError(e).into_response(),
    };
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let query = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("t=")).map(str::to_string));
    if bearer.or(query).as_deref() == Some(expected.as_str()) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, Json(json!({"error": "not paired"}))).into_response()
    }
}

async fn pair_local(State(svc): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if !is_local_client(&addr, &headers) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "pair with the QR code from the desktop app"}))).into_response();
    }
    match token(&svc) {
        Ok(t) => Json(json!({ "token": t })).into_response(),
        Err(e) => ApiError(e).into_response(),
    }
}

async fn health(State(svc): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "app": "pic_yomi", "version": env!("CARGO_PKG_VERSION"), "ocr_ready": svc.ocr_ready() }))
}

#[derive(Deserialize)]
struct OcrReq {
    /// Base64 or data URL.
    image: String,
    key: String,
}

async fn ocr(State(svc): State<AppState>, Json(r): Json<OcrReq>) -> ApiResult<crate::ocr::OcrPage> {
    if let Some(hit) = svc.ocr_cached(&r.key) {
        if r.image.is_empty() {
            return Ok(Json(hit));
        }
    }
    let data = r.image.split_once(',').map(|(_, d)| d.to_string()).unwrap_or(r.image);
    let page = tokio::task::spawn_blocking(move || -> Result<_> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim())?;
        svc.ocr_bytes(&r.key, &bytes)
    })
    .await??;
    Ok(Json(page))
}

#[derive(Deserialize)]
struct LookupReq {
    text: String,
    #[serde(default)]
    offset: usize,
}

async fn lookup(State(svc): State<AppState>, Json(r): Json<LookupReq>) -> ApiResult<crate::service::LookupResponse> {
    Ok(Json(tokio::task::spawn_blocking(move || svc.lookup(&r.text, r.offset)).await??))
}

async fn explain_word(State(svc): State<AppState>, Json(r): Json<ExplainRequest>) -> ApiResult<crate::cards::Explanation> {
    Ok(Json(explain::explain(&svc.db, &r).await?))
}

async fn get_llm(State(svc): State<AppState>) -> ApiResult<LlmConfig> {
    Ok(Json(LlmConfig::load(&svc.db)?.redacted()))
}

async fn put_llm(State(svc): State<AppState>, Json(mut cfg): Json<LlmConfig>) -> ApiResult<LlmConfig> {
    let old = LlmConfig::load(&svc.db)?;
    if cfg.api_key.contains('…') {
        cfg.api_key = old.api_key; // unchanged masked value
    }
    cfg.save(&svc.db)?;
    Ok(Json(cfg.redacted()))
}

async fn get_tts(State(svc): State<AppState>) -> ApiResult<audio::TtsConfig> {
    Ok(Json(audio::TtsConfig::load(&svc.db)?.redacted()))
}

async fn put_tts(State(svc): State<AppState>, Json(cfg): Json<audio::TtsConfig>) -> ApiResult<audio::TtsConfig> {
    cfg.save(&svc.db)?;
    Ok(Json(audio::TtsConfig::load(&svc.db)?.redacted()))
}

#[derive(Deserialize)]
struct AudioQuery {
    /// Word and its reading.
    #[serde(default)]
    e: String,
    #[serde(default)]
    r: String,
    /// A sentence to read instead of the word.
    #[serde(default)]
    s: String,
}

async fn audio(State(svc): State<AppState>, Query(q): Query<AudioQuery>) -> Response {
    let res = if q.s.trim().is_empty() { audio::word(&svc.db, &q.e, &q.r).await } else { audio::sentence(&svc.db, &q.s, &q.e, &q.r).await };
    match res {
        Ok(a) => (
            [
                (header::CONTENT_TYPE, a.mime),
                (header::CACHE_CONTROL, "private, max-age=604800".into()),
                (header::HeaderName::from_static("x-audio-source"), a.source),
            ],
            a.bytes,
        )
            .into_response(),
        Err(e) => ApiError(e).into_response(),
    }
}

async fn add_card(State(svc): State<AppState>, Json(r): Json<AddCardRequest>) -> ApiResult<Card> {
    let svc2 = svc.clone();
    let card = tokio::task::spawn_blocking(move || svc2.add_card(&r)).await??;
    {
        let (db, e, r, sentence) = (svc.db.clone(), card.expression.clone(), card.reading.clone(), card.sentence.clone());
        tokio::spawn(async move { audio::prefetch(&db, &e, &r, &sentence).await });
    }
    if card.explanation.is_none() {
        let id = card.id.clone();
        tokio::spawn(async move {
            if let Err(e) = svc.fill_explanation(&id).await {
                eprintln!("explanation for {id}: {e:#}");
            }
        });
    }
    Ok(Json(card))
}

async fn list_cards(State(svc): State<AppState>) -> ApiResult<Vec<cards::CardWithSchedule>> {
    Ok(Json(tokio::task::spawn_blocking(move || svc.cards()).await??))
}

async fn delete_card(State(svc): State<AppState>, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    svc.delete_card(&id)?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct ExistsQ {
    expression: String,
    #[serde(default)]
    reading: String,
}

async fn card_exists(State(svc): State<AppState>, Query(q): Query<ExistsQ>) -> ApiResult<serde_json::Value> {
    let c = cards::find_card(&svc.db, &q.expression, &q.reading)?;
    Ok(Json(json!({ "card_id": c.map(|c| c.id) })))
}

#[derive(Deserialize)]
struct ReviewReq {
    card_id: String,
    rating: u8,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    duration_ms: Option<i64>,
}

async fn review(State(svc): State<AppState>, Json(r): Json<ReviewReq>) -> ApiResult<crate::srs::Schedule> {
    Ok(Json(svc.review(&r.card_id, r.rating, &r.device_id, r.duration_ms)?))
}

async fn queue(State(svc): State<AppState>) -> ApiResult<Vec<crate::service::QueueItem>> {
    Ok(Json(tokio::task::spawn_blocking(move || svc.queue()).await??))
}

async fn stats(State(svc): State<AppState>) -> ApiResult<crate::cards::Stats> {
    Ok(Json(tokio::task::spawn_blocking(move || svc.stats()).await??))
}

async fn sync(State(svc): State<AppState>, Json(r): Json<SyncRequest>) -> ApiResult<cards::SyncResponse> {
    Ok(Json(tokio::task::spawn_blocking(move || cards::sync(&svc.db, &r)).await??))
}

async fn srs_settings(State(svc): State<AppState>) -> ApiResult<serde_json::Value> {
    let s = svc.scheduler();
    let tts = audio::TtsConfig::load(&svc.db)?;
    Ok(Json(json!({
        "desired_retention": s.config.desired_retention,
        "new_per_day": svc.new_cards_per_day(),
        "four_buttons": svc.four_buttons(),
        "audio_autoplay": tts.autoplay,
        "voice": if tts.azure_ready() { tts.voice } else { "macos".into() },
    })))
}

async fn media(State(svc): State<AppState>, Path(id): Path<String>) -> Response {
    match cards::load_media(&svc.db, &id) {
        Ok(Some((bytes, mime))) => {
            ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, max-age=31536000, immutable".into())], bytes)
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError(e).into_response(),
    }
}

async fn dicts(State(svc): State<AppState>) -> ApiResult<Vec<dict::DictInfo>> {
    Ok(Json(dict::list(&svc.db)?))
}

/// Where the phone web app's built files live.
pub fn web_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("PICYOMI_WEB").map(PathBuf::from),
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("../Resources/web"))),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist")),
    ];
    candidates.into_iter().flatten().find(|p| p.join("index.html").is_file())
}

pub fn router(svc: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/api/pair-local", get(pair_local))
        .route("/api/ocr", post(ocr))
        .route("/api/lookup", post(lookup))
        .route("/api/explain", post(explain_word))
        .route("/api/settings/llm", get(get_llm).put(put_llm))
        .route("/api/settings/srs", get(srs_settings))
        .route("/api/settings/tts", get(get_tts).put(put_tts))
        .route("/api/audio", get(audio))
        .route("/api/cards", get(list_cards).post(add_card))
        .route("/api/cards/exists", get(card_exists))
        .route("/api/cards/{id}", axum::routing::delete(delete_card))
        .route("/api/review", post(review))
        .route("/api/queue", get(queue))
        .route("/api/stats", get(stats))
        .route("/api/sync", post(sync))
        .route("/api/media/{id}", get(media))
        .route("/api/dicts", get(dicts));
    let mut app = api;
    if let Some(dir) = web_dir() {
        let index = dir.join("index.html");
        app = app.nest_service(
            "/app",
            tower_http::services::ServeDir::new(&dir).fallback(tower_http::services::ServeFile::new(index)),
        );
    }
    app.layer(middleware::from_fn_with_state(svc.clone(), auth))
        .layer(DefaultBodyLimit::max(40 * 1024 * 1024))
        .with_state(svc)
}

/// Serve on 127.0.0.1:`port` until the process exits.
pub async fn serve(svc: AppState, port: u16) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, router(svc).into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
