use crate::{
    config::{Settings, validate_listener},
    model::Dashboard,
    monitor,
    store::Store,
};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use fs2::FileExt;
use serde::Deserialize;
use std::{fs::OpenOptions, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, RwLock, watch};

#[derive(Clone)]
struct AppState {
    dashboard: Arc<RwLock<Dashboard>>,
    settings: Arc<RwLock<Settings>>,
    config_path: PathBuf,
    token: Option<String>,
    listener: SocketAddr,
    stop: watch::Sender<bool>,
    mutations: Arc<Mutex<()>>,
}
#[derive(Default)]
#[non_exhaustive]
pub struct ServiceOptions {
    pub database: PathBuf,
    pub config: PathBuf,
    pub bind: Option<SocketAddr>,
    pub token: Option<String>,
    pub max_samples: Option<u64>,
}

fn host_allowed(host: &str, address: SocketAddr) -> bool {
    let Ok(url) = reqwest::Url::parse(&format!("http://{host}")) else {
        return false;
    };
    url.port_or_known_default() == Some(address.port())
        && matches!(
            url.host_str().unwrap_or(""),
            "localhost" | "127.0.0.1" | "[::1]" | "::1"
        )
}
async fn guard(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let mut denial = None;
    let host = request
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if state.listener.ip().is_loopback() && !host_allowed(host, state.listener) {
        denial = Some(StatusCode::FORBIDDEN);
    }
    if let Some(origin) = request.headers().get("origin") {
        let matches = origin
            .to_str()
            .ok()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .is_some_and(|origin| {
                reqwest::Url::parse(&format!("http://{host}")).is_ok_and(|local| {
                    origin.host_str() == local.host_str()
                        && origin.port_or_known_default() == local.port_or_known_default()
                        && matches!(origin.scheme(), "http" | "https")
                })
            });
        if !matches {
            denial = Some(StatusCode::FORBIDDEN);
        }
    }
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        denial = Some(StatusCode::FORBIDDEN);
    }
    if request.uri().path().starts_with("/api/")
        && request.uri().path() != "/api/health"
        && let Some(token) = &state.token
    {
        let expected = format!("Bearer {token}");
        let received = request
            .headers()
            .get("authorization")
            .map(HeaderValue::as_bytes)
            .unwrap_or_default();
        if received.len() != expected.len()
            || received
                .iter()
                .zip(expected.as_bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                != 0
        {
            denial = Some(StatusCode::UNAUTHORIZED);
        }
    }
    let mut response = if let Some(status) = denial {
        (status, Json(serde_json::json!({"error":"Access denied"}))).into_response()
    } else {
        next.run(request).await
    };
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}
async fn dashboard(State(state): State<AppState>) -> Json<Dashboard> {
    Json(state.dashboard.read().await.clone())
}
async fn settings(State(state): State<AppState>) -> Json<Settings> {
    Json(state.settings.read().await.clone())
}
type ApiResult =
    std::result::Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)>;
fn api_error(error: anyhow::Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error":error.to_string()})),
    )
}
async fn save_settings(state: &AppState, value: Settings) -> Result<()> {
    value.validate()?;
    let path = state.config_path.clone();
    let copy = value.clone();
    tokio::task::spawn_blocking(move || copy.save(&path)).await??;
    *state.settings.write().await = value;
    Ok(())
}
async fn update_settings(State(state): State<AppState>, Json(value): Json<Settings>) -> ApiResult {
    let _serial = state.mutations.lock().await;
    let mut value = value;
    if !value.preset.matches(&value) {
        value.preset = crate::presets::Preset::Custom;
    }
    save_settings(&state, value).await.map_err(api_error)?;
    let value = state.settings.read().await;
    monitor::refresh_profile(&mut *state.dashboard.write().await, &value).map_err(api_error)?;
    Ok(Json(serde_json::json!({"saved":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetRequest {
    preset: crate::presets::Preset,
}
async fn apply_preset(
    State(state): State<AppState>,
    Json(request): Json<PresetRequest>,
) -> ApiResult {
    let _serial = state.mutations.lock().await;
    let mut value = state.settings.read().await.clone();
    request.preset.apply(&mut value).map_err(api_error)?;
    save_settings(&state, value).await.map_err(api_error)?;
    let value = state.settings.read().await;
    let mut view = state.dashboard.write().await;
    monitor::refresh_profile(&mut view, &value).map_err(api_error)?;
    Ok(Json(
        serde_json::json!({"saved":true,"preset":value.preset}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    paused: Option<bool>,
    gaming: Option<bool>,
}
async fn control(State(state): State<AppState>, Json(control): Json<Control>) -> ApiResult {
    let _serial = state.mutations.lock().await;
    let mut value = state.settings.read().await.clone();
    if let Some(paused) = control.paused {
        value.paused = paused;
    }
    if let Some(gaming) = control.gaming {
        value.gaming = gaming;
    }
    save_settings(&state, value).await.map_err(api_error)?;
    let value = state.settings.read().await;
    let mut view = state.dashboard.write().await;
    monitor::refresh_profile(&mut view, &value).map_err(api_error)?;
    Ok(Json(serde_json::json!({"saved":true})))
}
async fn shutdown(State(state): State<AppState>) -> Json<serde_json::Value> {
    let _ = state.stop.send(true);
    Json(serde_json::json!({"stopping":true}))
}
async fn export(State(state): State<AppState>) -> Response {
    let view = state.dashboard.read().await.clone();
    let body = serde_json::to_string_pretty(&serde_json::json!({"schema_version":1,"exported_at":chrono::Utc::now(),"scope":"Last 120 samples, 100 incidents, and 24 hourly summaries. Use CLI export for full retained history.","privacy":"Review local addresses and ISP labels before sharing.","dashboard":view})).unwrap_or_default();
    (
        [
            ("content-type", "application/json"),
            (
                "content-disposition",
                "attachment; filename=aujitter-evidence.json",
            ),
        ],
        body,
    )
        .into_response()
}
pub async fn serve(options: ServiceOptions) -> Result<()> {
    let bind = options
        .bind
        .unwrap_or_else(|| "127.0.0.1:9876".parse().unwrap());
    validate_listener(bind, options.token.as_deref())?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .context("Could not listen; another monitor may already be running")?;
    let configured = Settings::load(&options.config)?;
    if !options.config.exists() {
        configured.save(&options.config)?;
    }
    if let Some(parent) = options
        .database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(options.database.with_extension("lock"))?;
    lock.try_lock_exclusive()
        .context("Another AuJitter monitor is already writing this database")?;
    let store = Arc::new(Store::open(&options.database)?);
    let settings = Arc::new(RwLock::new(configured.clone()));
    let view = Arc::new(RwLock::new(monitor::initial_dashboard(&configured)));
    {
        let mut initial = view.write().await;
        initial.recent = store.samples(120)?;
        initial.latest = store.latest()?;
        initial.incidents = store.incidents(100)?;
        initial.hourly = store.hourly(24)?;
    }
    let (stop, stop_rx) = watch::channel(false);
    let state = AppState {
        dashboard: view.clone(),
        settings: settings.clone(),
        config_path: options.config,
        token: options.token,
        listener: listener.local_addr()?,
        stop: stop.clone(),
        mutations: Arc::new(Mutex::new(())),
    };
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [("content-type", "text/javascript")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [("content-type", "text/css")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route(
            "/api/health",
            get(|| async {
                Json(serde_json::json!({"app":"aujitter","version":env!("CARGO_PKG_VERSION")}))
            }),
        )
        .route("/api/dashboard", get(dashboard))
        .route("/api/settings", get(self::settings).post(update_settings))
        .route(
            "/api/presets",
            get(|| async { Json(crate::presets::catalogue()) }),
        )
        .route("/api/preset", post(apply_preset))
        .route("/api/control", post(control))
        .route("/api/export", get(export))
        .route("/api/shutdown", post(shutdown))
        .layer(DefaultBodyLimit::max(16384))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state);
    let mut monitor_task = tokio::spawn(monitor::run(
        store,
        settings,
        view,
        stop_rx.clone(),
        options.max_samples,
    ));
    let mut web_task = tokio::spawn(async move {
        let mut shutdown = stop_rx;
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while !*shutdown.borrow() {
                    if shutdown.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await
    });
    let result = tokio::select! {
        result = &mut monitor_task => { stop.send(true).ok(); web_task.await??; result??; Ok(()) },
        result = &mut web_task => { stop.send(true).ok(); monitor_task.await??; result??; Ok(()) },
        result = shutdown_signal() => { result?; stop.send(true).ok(); monitor_task.await??; web_task.await??; Ok(()) }
    };
    FileExt::unlock(&lock)?;
    result
}
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn concurrent_controls_preserve_both_preferences_and_the_saved_file() {
        let temporary = tempfile::tempdir().unwrap();
        let cfg = Settings::default();
        let (stop, _) = watch::channel(false);
        let state = AppState {
            dashboard: Arc::new(RwLock::new(monitor::initial_dashboard(&cfg))),
            settings: Arc::new(RwLock::new(cfg)),
            config_path: temporary.path().join("settings.json"),
            token: None,
            listener: "127.0.0.1:9876".parse().unwrap(),
            stop,
            mutations: Arc::new(Mutex::new(())),
        };
        let (a, b) = tokio::join!(
            control(
                State(state.clone()),
                Json(Control {
                    paused: Some(true),
                    gaming: None
                })
            ),
            control(
                State(state.clone()),
                Json(Control {
                    paused: None,
                    gaming: Some(true)
                })
            )
        );
        assert!(a.is_ok() && b.is_ok());
        let saved = Settings::load(&state.config_path).unwrap();
        assert!(saved.paused && saved.gaming);
    }
    #[test]
    fn rejects_dns_rebinding_hostnames() {
        let listen = "127.0.0.1:9876".parse().unwrap();
        assert!(host_allowed("127.0.0.1:9876", listen));
        assert!(host_allowed("localhost:9876", listen));
        assert!(!host_allowed("evil.example:9876", listen));
        assert!(!host_allowed("localhost:80", listen));
    }
}
