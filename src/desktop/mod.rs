//! Native presentation of the same local monitoring service used by the CLI and web UI.
mod instance;
mod tray;

use crate::{
    config::{self, Settings},
    model::{Dashboard, ProbeKind, Severity},
    presets::{self, Preset},
    startup,
};
use anyhow::{Context as _, Result};
use chrono::Local;
use gpui_kit::component::{
    ActiveTheme, IndexPath, Selectable, Theme, ThemeMode,
    button::{Button, ButtonVariants},
    chart::LineChart,
    form::{Field, Form},
    input::{Input, InputState},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
};
use gpui_kit::*;
use instance::DesktopInstance;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
use tray::{Tray, TrayAction};

actions!(
    aujitter,
    [TogglePause, ToggleGaming, OpenWeb, ExportEvidence, Quit]
);

#[derive(Default, Clone)]
struct Live {
    revision: u64,
    view: Option<Dashboard>,
    preferences: Option<Settings>,
    notice: String,
    error: Option<String>,
    startup: bool,
    connected: bool,
    should_quit: bool,
}
enum Operation {
    Control(serde_json::Value),
    Save(Settings),
    Preset(Preset),
    Export,
    Startup(bool),
    Restart,
    Shutdown,
}

#[derive(Clone, Copy)]
enum DesktopEvent {
    LiveChanged,
    OperationFinished,
    Tray(TrayAction),
}

struct DesktopSession(Entity<DesktopState>);
impl Global for DesktopSession {}

/// The session owns the worker and tray; closing a presentation window drops neither.
struct DesktopState {
    live: Live,
    commands: SyncSender<Operation>,
    tray: Option<Tray>,
    tray_error: Option<String>,
    quitting: bool,
    _events: Task<()>,
}

impl DesktopState {
    fn new(request_path: PathBuf, support: Result<()>, cx: &mut Context<Self>) -> Self {
        let (events, incoming) = async_channel::bounded(16);
        let (tray, tray_error) = match support.and_then(|_| Tray::new(events.clone())) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (
                None,
                Some(format!(
                    "System tray unavailable: {error}. The monitor can still run in the background."
                )),
            ),
        };
        tracing::info!(
            tray_available = tray.is_some(),
            "AuJitter desktop session started"
        );
        let live = Live {
            notice: if tray.is_some() {
                "Closing the window keeps monitoring in the system tray.".into()
            } else {
                String::new()
            },
            ..Default::default()
        };
        let shared = Arc::new(Mutex::new(live.clone()));
        let (commands, receiver) = mpsc::sync_channel(8);
        let observed = shared.clone();
        std::thread::spawn(move || worker(shared, receiver, events, request_path));
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = incoming.recv().await {
                // Window creation reads this session from the new view. Perform it outside
                // the session's entity update, so neither state nor deferred effects are leased.
                if matches!(event, DesktopEvent::Tray(TrayAction::Open)) {
                    cx.update(|cx| {
                        if let Some(session) = this.upgrade() {
                            show_window(session, cx);
                        }
                    });
                    continue;
                }
                let snapshot = observed.lock().unwrap().clone();
                if let Err(error) = this.update(cx, |session, cx| match event {
                    DesktopEvent::Tray(action) => session.handle_action(action, cx),
                    DesktopEvent::LiveChanged | DesktopEvent::OperationFinished => {
                        let changed = session.live.revision != snapshot.revision;
                        if matches!(event, DesktopEvent::OperationFinished) {
                            session.quitting = false;
                        }
                        session.live = snapshot;
                        if session.live.should_quit {
                            cx.quit();
                            return;
                        }
                        session.refresh_tray();
                        if changed || matches!(event, DesktopEvent::OperationFinished) {
                            cx.notify();
                        }
                    }
                }) {
                    tracing::error!(%error, "Desktop event session ended");
                    break;
                }
            }
        });
        Self {
            live,
            commands,
            tray,
            tray_error,
            quitting: false,
            _events: task,
        }
    }

    fn snapshot(&self) -> Live {
        let mut live = self.live.clone();
        if let Some(error) = &self.tray_error {
            live.error = Some(match live.error {
                Some(other) => format!("{other} {error}"),
                None => error.clone(),
            });
        }
        live
    }

    fn refresh_tray(&mut self) {
        if let Some(tray) = &mut self.tray {
            tray.update(&self.live, self.quitting);
        }
    }

    fn handle_action(&mut self, action: TrayAction, cx: &mut Context<Self>) {
        tracing::debug!(?action, "Desktop command received");
        let operation = match action {
            TrayAction::Open => {
                let session = cx.entity();
                cx.defer(move |cx| show_window(session, cx));
                return;
            }
            TrayAction::Web => {
                cx.open_url("http://127.0.0.1:9876");
                return;
            }
            _ if self.quitting => return,
            TrayAction::Pause => {
                let Some(view) = self.live.view.as_ref().filter(|_| self.live.connected) else {
                    return;
                };
                Operation::Control(serde_json::json!({"paused": !view.paused}))
            }
            TrayAction::Gaming => {
                let Some(view) = self.live.view.as_ref().filter(|_| self.live.connected) else {
                    return;
                };
                Operation::Control(serde_json::json!({"gaming": !view.gaming}))
            }
            TrayAction::Restart => Operation::Restart,
            TrayAction::Quit => Operation::Shutdown,
        };
        let quitting = matches!(&operation, Operation::Shutdown);
        match self.commands.try_send(operation) {
            Ok(()) => self.quitting = quitting,
            Err(error) => self.live.error = Some(format!("Command not sent: {error}")),
        }
        self.refresh_tray();
        cx.notify();
    }
}

fn monitor_executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "aujitter.exe"
    } else {
        "aujitter"
    }))
}
fn start_monitor() -> Result<()> {
    let mut command = Command::new(monitor_executable()?);
    command
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let status = command.status().context("Could not start the companion monitor. Keep aujitter and aujitter-gui in the same directory.")?;
    anyhow::ensure!(
        status.success(),
        "The monitor could not start. Inspect the monitor log or check whether port 9876 is in use."
    );
    Ok(())
}

fn worker(
    shared: Arc<Mutex<Live>>,
    commands: mpsc::Receiver<Operation>,
    events: async_channel::Sender<DesktopEvent>,
    request_path: PathBuf,
) {
    let publish = |event| {
        let _ = events.send_blocking(event);
    };
    let token = std::env::var("AUJITTER_TOKEN").ok();
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .no_proxy()
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            let mut live = shared.lock().unwrap();
            live.error = Some(error.to_string());
            live.revision += 1;
            drop(live);
            publish(DesktopEvent::LiveChanged);
            return;
        }
    };
    if let Err(error) = start_monitor() {
        let mut live = shared.lock().unwrap();
        live.error = Some(error.to_string());
        live.revision += 1;
        drop(live);
        publish(DesktopEvent::LiveChanged);
    }
    let request = |route: &str| {
        let request = client.get(format!("http://127.0.0.1:9876/api/{route}"));
        if let Some(token) = &token {
            request.bearer_auth(token)
        } else {
            request
        }
    };
    let refresh = || -> Result<(Dashboard, Settings)> {
        Ok((
            request("dashboard").send()?.error_for_status()?.json()?,
            request("settings").send()?.error_for_status()?.json()?,
        ))
    };
    let mut next_refresh = Instant::now();
    loop {
        // Local IPC is only a request to show the existing window. All disk/network I/O stays here.
        if std::fs::remove_file(&request_path).is_ok() {
            tracing::debug!("Existing desktop requested to open its window");
            publish(DesktopEvent::Tray(TrayAction::Open));
        }
        if Instant::now() >= next_refresh {
            match refresh() {
                Ok((view, preferences)) => {
                    let start_at_sign_in = startup::is_enabled();
                    let mut live = shared.lock().unwrap();
                    live.view = Some(view);
                    live.preferences = Some(preferences);
                    live.error = None;
                    live.connected = true;
                    live.revision += 1;
                    live.startup = start_at_sign_in;
                }
                Err(error) => {
                    let mut live = shared.lock().unwrap();
                    live.error = Some(format!(
                        "Monitor unavailable: {error}. Last observations may be stale."
                    ));
                    live.revision += 1;
                    live.connected = false;
                }
            }
            publish(DesktopEvent::LiveChanged);
            next_refresh = Instant::now() + Duration::from_secs(5);
        }
        let wait = next_refresh
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(1));
        let operation = match commands.recv_timeout(wait) {
            Ok(operation) => operation,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let shutting_down = matches!(&operation, Operation::Shutdown);
        let result: Result<String> = (|| match operation {
            Operation::Control(payload) => {
                let mut request = client
                    .post("http://127.0.0.1:9876/api/control")
                    .json(&payload);
                if let Some(token) = &token {
                    request = request.bearer_auth(token);
                }
                request.send()?.error_for_status()?;
                Ok("Monitoring setting saved".into())
            }
            Operation::Preset(preset) => {
                let mut call = client
                    .post("http://127.0.0.1:9876/api/preset")
                    .json(&serde_json::json!({"preset":preset}));
                if let Some(token) = &token {
                    call = call.bearer_auth(token);
                }
                call.send()?.error_for_status()?;
                Ok(format!("{} preset applied", preset.name()))
            }
            Operation::Save(mut value) => {
                let current: Settings = request("settings").send()?.error_for_status()?.json()?;
                value.paused = current.paused;
                value.gaming = current.gaming;
                value.validate()?;
                let mut request = client
                    .post("http://127.0.0.1:9876/api/settings")
                    .json(&value);
                if let Some(token) = &token {
                    request = request.bearer_auth(token);
                }
                let response = request.send()?;
                if !response.status().is_success() {
                    anyhow::bail!(
                        "{}",
                        response
                            .json::<serde_json::Value>()?
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Could not save preferences")
                    );
                }
                Ok("Preferences saved".into())
            }
            Operation::Export => {
                let text = request("export").send()?.error_for_status()?.text()?;
                let folder = config::data_dir().join("exports");
                std::fs::create_dir_all(&folder)?;
                let file = folder.join(format!(
                    "aujitter-evidence-{}.json",
                    Local::now().format("%Y%m%d-%H%M%S")
                ));
                std::fs::write(&file, text)?;
                Ok(format!("Evidence saved to {}", file.display()))
            }
            Operation::Startup(enabled) => {
                startup::set_enabled(enabled, std::env::current_exe()?)?;
                Ok(if enabled {
                    "Start at sign-in enabled"
                } else {
                    "Start at sign-in disabled"
                }
                .into())
            }
            Operation::Restart => {
                start_monitor()?;
                Ok("Monitor started".into())
            }
            Operation::Shutdown => {
                let mut call = client.post("http://127.0.0.1:9876/api/shutdown");
                if let Some(token) = &token {
                    call = call.bearer_auth(token);
                }
                match call.send() {
                    Ok(response) => {
                        response.error_for_status()?;
                    }
                    Err(error) if error.is_connect() => {} // An already stopped monitor needs no shutdown.
                    Err(error) => return Err(error.into()),
                }
                Ok("Monitoring stopped".into())
            }
        })();
        let mut live = shared.lock().unwrap();
        match result {
            Ok(notice) => {
                live.notice = notice;
                live.error = None;
                live.should_quit = shutting_down;
            }
            Err(error) => live.error = Some(error.to_string()),
        };
        live.revision += 1;
        let should_quit = live.should_quit;
        drop(live);
        publish(DesktopEvent::OperationFinished);
        if should_quit {
            break;
        }
        next_refresh = Instant::now();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Monitor,
    History,
    Preferences,
}
struct NetworkWindow {
    session: Entity<DesktopState>,
    commands: SyncSender<Operation>,
    live: Live,
    page: Page,
    focus: FocusHandle,
    isp: Entity<InputState>,
    gateway: Entity<InputState>,
    dns: Entity<InputState>,
    interval: Entity<InputState>,
    gaming_interval: Entity<InputState>,
    latency: Entity<InputState>,
    jitter: Entity<InputState>,
    preset: Entity<SelectState<Vec<SharedString>>>,
    _preset_subscription: Subscription,
    discover_router: bool,
    _live_subscription: Subscription,
}

fn metric(title: &str, value: String, caption: &str, cx: &App) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .bg(cx.theme().muted)
        .rounded(cx.theme().radius)
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(title.to_string()),
        )
        .child(div().text_2xl().child(value))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(caption.to_string()),
        )
}
fn measured(value: Option<f64>, unit: &str) -> String {
    value
        .map(|v| format!("{v:.1}{unit}"))
        .unwrap_or_else(|| "—".into())
}
fn time(value: chrono::DateTime<chrono::Utc>) -> String {
    value
        .with_timezone(&Local)
        .format("%d %b %H:%M:%S")
        .to_string()
}

impl NetworkWindow {
    fn new(session: Entity<DesktopState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commands = session.read(cx).commands.clone();
        let live = session.read(cx).snapshot();
        let subscription = cx.observe(&session, |view, session, cx| {
            view.live = session.read(cx).snapshot();
            cx.notify();
        });
        let mut input =
            |placeholder: &str| cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let isp = input("ISP label (optional)");
        let gateway = input("Automatically detect");
        let dns = input("Automatically detect");
        let interval = input("5");
        let gaming_interval = input("15");
        let latency = input("150");
        let jitter = input("30");
        let preset = cx.new(|cx| {
            SelectState::new(
                presets::catalogue()
                    .iter()
                    .map(|item| SharedString::from(item.name))
                    .collect::<Vec<_>>(),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let preset_subscription = cx
            .subscribe(&preset, |_, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                cx.notify()
            });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            session,
            commands,
            live,
            page: Page::Monitor,
            focus,
            isp,
            gateway,
            dns,
            interval,
            gaming_interval,
            latency,
            jitter,
            preset,
            _preset_subscription: preset_subscription,
            discover_router: false,
            _live_subscription: subscription,
        }
    }
    fn send(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if self.commands.try_send(operation).is_err() {
            self.live.error = Some("The monitor is busy. Try again shortly.".into());
        } else {
            self.live.notice = "Applying change…".into();
        }
        cx.notify();
    }
    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.handle_action(TrayAction::Pause, cx)
        });
    }
    fn toggle_gaming(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.handle_action(TrayAction::Gaming, cx)
        });
    }
    fn open_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(settings) = &self.live.preferences {
            let name = SharedString::from(settings.preset.name());
            self.preset
                .update(cx, |state, cx| state.set_selected_value(&name, window, cx));
            for (input, value) in [
                (&self.isp, settings.isp.clone().unwrap_or_default()),
                (
                    &self.gateway,
                    settings.gateway.map(|v| v.to_string()).unwrap_or_default(),
                ),
                (
                    &self.dns,
                    settings
                        .dns_server
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                ),
                (&self.interval, (settings.interval_ms / 1000).to_string()),
                (
                    &self.gaming_interval,
                    (settings.gaming_interval_ms / 1000).to_string(),
                ),
                (&self.latency, settings.latency_warning_ms.to_string()),
                (&self.jitter, settings.jitter_warning_ms.to_string()),
            ] {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            self.discover_router = settings.discover_router;
        }
        self.page = Page::Preferences;
        cx.notify();
    }
    fn apply_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.preset.read(cx).selected_value() else {
            return;
        };
        let Some(preset) = presets::catalogue()
            .into_iter()
            .find(|p| p.name == selected.as_ref())
        else {
            return;
        };
        let Some(mut settings) = self.live.preferences.clone() else {
            return;
        };
        match preset.id.apply(&mut settings) {
            Ok(()) => {
                self.live.preferences = Some(settings.clone());
                self.open_preferences(window, cx);
                self.send(Operation::Preset(preset.id), cx);
            }
            Err(error) => {
                self.live.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn save_preferences(&mut self, cx: &mut Context<Self>) {
        let result: Result<Settings> = (|| {
            let mut settings = self
                .live
                .preferences
                .clone()
                .context("Waiting for monitor preferences")?;
            let isp = self.isp.read(cx).value().to_string();
            settings.isp = (!isp.trim().is_empty()).then(|| isp.trim().into());
            let gateway = self.gateway.read(cx).value().to_string();
            settings.gateway = if gateway.trim().is_empty() {
                None
            } else {
                Some(
                    gateway
                        .trim()
                        .parse()
                        .context("Router IP must be an IPv4 or IPv6 address")?,
                )
            };
            let dns = self.dns.read(cx).value().to_string();
            settings.dns_server = if dns.trim().is_empty() {
                None
            } else {
                Some(
                    dns.trim()
                        .parse()
                        .context("DNS IP must be an IPv4 or IPv6 address")?,
                )
            };
            settings.interval_ms = self
                .interval
                .read(cx)
                .value()
                .parse::<u64>()
                .context("Normal interval must be a whole number of seconds")?
                .checked_mul(1000)
                .context("Normal interval is too large")?;
            settings.gaming_interval_ms = self
                .gaming_interval
                .read(cx)
                .value()
                .parse::<u64>()
                .context("Gaming interval must be a whole number of seconds")?
                .checked_mul(1000)
                .context("Gaming interval is too large")?;
            settings.latency_warning_ms = self
                .latency
                .read(cx)
                .value()
                .parse()
                .context("Latency warning must be a number")?;
            settings.jitter_warning_ms = self
                .jitter
                .read(cx)
                .value()
                .parse()
                .context("Jitter warning must be a number")?;
            settings.discover_router = self.discover_router;
            settings.validate()?;
            Ok(settings)
        })();
        match result {
            Ok(settings) => self.send(Operation::Save(settings), cx),
            Err(error) => {
                self.live.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn monitor(&self, cx: &App) -> Div {
        let mut content = div().flex().flex_col().gap_6();
        let Some(view) = &self.live.view else {
            return content.child("Connecting to the local monitoring service…");
        };
        let Some(sample) = &view.latest else {
            return content.child("Collecting the first observation…");
        };
        let diagnosis = &sample.diagnosis;
        let colour = match diagnosis.severity {
            Severity::Offline => cx.theme().danger,
            Severity::Watch | Severity::Degraded => cx.theme().warning,
            Severity::Stable => cx.theme().success,
            _ => cx.theme().muted_foreground,
        };
        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_3xl()
                        .text_color(if view.paused {
                            cx.theme().muted_foreground
                        } else {
                            colour
                        })
                        .child(if view.paused {
                            "Paused"
                        } else {
                            diagnosis.severity.label()
                        }),
                )
                .child(if view.paused {
                    "No probes are being sent. Resume monitoring to collect observations."
                        .to_string()
                } else {
                    diagnosis.explanation.clone()
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "Confidence: {} · Last observation {} · {}s interval",
                            diagnosis.confidence,
                            time(sample.at),
                            view.interval_ms / 1000
                        )),
                ),
        );
        let total: u64 = view.hourly.iter().map(|s| s.samples).sum();
        let unstable: u64 = view.hourly.iter().map(|s| s.unstable_samples).sum();
        content = content.child(
            div()
                .flex()
                .gap_3()
                .child(metric(
                    "Internet latency",
                    measured(sample.metrics.latency_ms, " ms"),
                    "Latest timed echo reply",
                    cx,
                ))
                .child(metric(
                    "Latency variation",
                    measured(sample.metrics.jitter_ms, " ms"),
                    "Mean consecutive difference",
                    cx,
                ))
                .child(metric(
                    "Echo failure",
                    measured(sample.metrics.icmp_loss_percent, "%"),
                    "Loss or filtering · 5 minutes",
                    cx,
                ))
                .child(metric(
                    "Unstable observations",
                    if total == 0 {
                        "—".into()
                    } else {
                        format!("{:.1}%", 100.0 * unstable as f64 / total as f64)
                    },
                    "Recorded samples · 24 hours",
                    cx,
                )),
        );
        // Each chart shows its latest uninterrupted run. Missing replies never become zeros or joined lines.
        let series = |gateway: bool| {
            let mut points: Vec<(String, f64)> = vec![];
            for row in view.recent.iter().rev() {
                let value = if gateway {
                    row.probes
                        .iter()
                        .find(|p| p.kind == ProbeKind::GatewayIcmp)
                        .and_then(|p| p.latency_ms)
                } else {
                    row.metrics.latency_ms
                };
                if row.observation_gap_seconds.is_some() {
                    break;
                }
                let Some(value) = value else {
                    break;
                };
                points.push((
                    row.at.with_timezone(&Local).format("%H:%M:%S").to_string(),
                    value,
                ));
                if points.len() >= 60 {
                    break;
                }
            }
            points.reverse();
            points
        };
        let mut charts = div().flex().gap_6();
        for (ix, name, gateway) in [
            (0usize, "Internet latency", false),
            (1, "Gateway latency", true),
        ] {
            let points = series(gateway);
            let mut panel = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_lg().child(name))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Latest uninterrupted timed replies · milliseconds"),
                );
            if points.len() >= 2 {
                panel = panel.child(
                    div().h_48().child(
                        LineChart::new(points)
                            .id(("latency-series", ix))
                            .x(|p| p.0.clone())
                            .y(|p| p.1)
                            .linear()
                            .stroke(if gateway {
                                cx.theme().success
                            } else {
                                cx.theme().primary
                            })
                            .name(name)
                            .y_axis(true)
                            .x_tick_count(3)
                            .y_tick_format(|v| format!("{v:.0}")),
                    ),
                );
            } else {
                panel = panel.child(
                    div()
                        .h_48()
                        .flex()
                        .items_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Waiting for two consecutive replies. Failures remain in the history.",
                        ),
                );
            }
            charts = charts.child(panel);
        }
        content = content.child(charts);
        let mut evidence = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_lg().child("Where the evidence points"));
        for item in diagnosis.evidence.iter().chain(&diagnosis.next_steps) {
            evidence = evidence.child(div().text_sm().child(item.clone()));
        }
        content = content.child(evidence);
        let topology = &sample.topology;
        let mut connection = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_lg().child("Connection details"));
        for (label, value) in [
            (
                "ISP",
                topology
                    .isp
                    .clone()
                    .unwrap_or_else(|| "Not configured".into()),
            ),
            ("Local connection", topology.local_transport.clone()),
            (
                "Gateway",
                topology
                    .gateway
                    .clone()
                    .unwrap_or_else(|| "Unavailable".into()),
            ),
            (
                "DNS",
                topology
                    .dns_server
                    .clone()
                    .unwrap_or_else(|| "Unavailable".into()),
            ),
            ("WAN type", format!("{:?}", topology.access_type)),
            (
                "Router",
                topology
                    .router_model
                    .clone()
                    .unwrap_or_else(|| "Not discovered".into()),
            ),
        ] {
            connection = connection.child(
                div()
                    .flex()
                    .gap_4()
                    .text_sm()
                    .child(
                        div()
                            .w_32()
                            .text_color(cx.theme().muted_foreground)
                            .child(label),
                    )
                    .child(value),
            );
        }
        content = content.child(
            connection.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(topology.access_evidence.clone()),
            ),
        );
        for note in &topology.notes {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(note.clone()),
            );
        }
        content
    }
    fn history(&self, cx: &App) -> Div {
        let mut content=div().flex().flex_col().gap_3().child(div().text_lg().child("Recorded incidents")).child(div().text_sm().text_color(cx.theme().muted_foreground).child("Up to 100 recent incidents. Default retention is 30 days of samples and one year of incidents. Three healthy observations close an incident."));
        if let Some(view) = &self.live.view {
            if view.incidents.is_empty() {
                content = content.child("No incidents recorded yet.");
            }
            for row in &view.incidents {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .py_3()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(format!(
                            "#{} · {} · {}",
                            row.id,
                            time(row.started_at),
                            row.peak_severity.label()
                        ))
                        .child(row.diagnosis.explanation.clone())
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} unstable observations · {}",
                                    row.unstable_samples,
                                    row.ended_at
                                        .map(|at| format!(
                                            "Ended {} · {}",
                                            time(at),
                                            row.end_reason.as_deref().unwrap_or("")
                                        ))
                                        .unwrap_or_else(|| "Ongoing".into())
                                )),
                        ),
                );
            }
        }
        content
    }
    fn preferences(&self, cx: &mut Context<Self>) -> Div {
        let selected = self.preset.read(cx).selected_value();
        let description = presets::catalogue()
            .into_iter()
            .find(|p| selected.is_some_and(|name| p.name == name.as_ref()))
            .map(|p| p.description)
            .unwrap_or("Choose a preset, or customise the preferences below.");
        let active = self
            .live
            .preferences
            .as_ref()
            .map(|s| s.preset)
            .unwrap_or(Preset::Automatic);
        div().flex().flex_col().gap_6()
            .child(div().text_lg().child(format!("Situation preset · {}", active.name())))
            .child(div().flex().gap_3().child(div().flex_1().child(Select::new(&self.preset).accessibility_label("Situation preset").placeholder("Choose a situation"))).child(Button::new("apply-preset").primary().label("Apply preset").on_click(cx.listener(|this,_,window,cx|this.apply_preset(window,cx)))))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(description))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(self.live.view.as_ref().filter(|_| active == Preset::Automatic).and_then(|view| view.preset_reason.clone()).unwrap_or_default()))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Presets change timing and warnings. They preserve your ISP, connection type, router/DNS overrides, history retention, router permission, and pause state."))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Automatic uses connection information only. Gaming remains a manual switch. Editing timing or warning values selects Custom preferences."))
            .child(Form::new().columns(2)
            .child(Field::new().label("ISP label").child(Input::new(&self.isp)))
            .child(Field::new().label("Router IP override").child(Input::new(&self.gateway)))
            .child(Field::new().label("DNS IP override").child(Input::new(&self.dns)))
            .child(Field::new().label("Normal interval (seconds)").child(Input::new(&self.interval)))
            .child(Field::new().label("Gaming interval (seconds)").child(Input::new(&self.gaming_interval)))
            .child(Field::new().label("Latency warning (ms)").child(Input::new(&self.latency)))
            .child(Field::new().label("Jitter warning (ms)").child(Input::new(&self.jitter)))
            .footer(Button::new("save-preferences").primary().label("Save preferences").on_click(cx.listener(|this,_,_,cx|this.save_preferences(cx)))))
            .child(Switch::new("discover-router").label("Read-only router discovery").checked(self.discover_router).on_change(cx.listener(|this,value,_,cx|{this.discover_router = *value;cx.notify();})))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Reads model and WAN status through UPnP when already available. Never changes router settings. Discovery is skipped in gaming mode. Save preferences to apply."))
            .child(Switch::new("startup").label("Start monitoring at sign-in").checked(self.live.startup).on_change(cx.listener(|this,value,_,cx|this.send(Operation::Startup(*value),cx))))
            .child(Button::new("advanced").outline().label("Connection type and advanced preferences…").on_click(|_,_,cx|cx.open_url("http://127.0.0.1:9876")))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Closing this window leaves the monitor running. Use Pause to suspend probes, or the CLI Stop command to stop the service."))
    }
}

impl Render for NetworkWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gaming = self.live.view.as_ref().is_some_and(|v| v.gaming);
        let paused = self.live.view.as_ref().is_some_and(|v| v.paused);
        let title = match self.page {
            Page::Monitor => "Network health",
            Page::History => "Incident history",
            Page::Preferences => "Settings",
        };
        let mut sidebar = div()
            .w_56()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(div().text_2xl().mb_2().child("AuJitter"));
        for (ix, label, page) in [
            (0usize, "Network health", Page::Monitor),
            (1, "Incident history", Page::History),
            (2, "Settings", Page::Preferences),
        ] {
            sidebar = sidebar.child(
                Button::new(("navigation", ix))
                    .ghost()
                    .label(label)
                    .selected(self.page == page)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if page == Page::Preferences {
                            this.open_preferences(window, cx);
                        } else {
                            this.page = page;
                            cx.notify();
                        }
                    })),
            );
        }
        sidebar = sidebar
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Stored on this device"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No cloud account required"),
            );
        let body = match self.page {
            Page::Monitor => self.monitor(cx),
            Page::History => self.history(cx),
            Page::Preferences => self.preferences(cx),
        };
        let mut content = div().flex().flex_col().flex_1().min_w_0().min_h_0().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .p_6()
                .gap_4()
                .child(div().text_2xl().child(title))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("gaming")
                                .outline()
                                .label("Gaming mode")
                                .selected(gaming)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_gaming(cx))),
                        )
                        .child(
                            Button::new("pause")
                                .outline()
                                .label(if paused { "Resume" } else { "Pause" })
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
                        )
                        .child(
                            Button::new("export")
                                .outline()
                                .label("Export evidence")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.send(Operation::Export, cx)),
                                ),
                        ),
                ),
        );
        if let Some(view) = &self.live.view {
            content = content.child(
                div()
                    .px_6()
                    .pb_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{}{} · every {}s{}",
                        if view.preset == Preset::Automatic {
                            "Automatic → "
                        } else {
                            ""
                        },
                        view.effective_preset.name(),
                        view.interval_ms / 1000,
                        if view.gaming {
                            " · Gaming mode on"
                        } else {
                            ""
                        }
                    )),
            );
            if let Some(reason) = &view.preset_reason {
                content = content.child(
                    div()
                        .px_6()
                        .pb_3()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(reason.clone()),
                );
            }
        }
        if let Some(error) = &self.live.error {
            content = content.child(
                div()
                    .px_6()
                    .pb_3()
                    .text_color(cx.theme().warning)
                    .child(error.clone()),
            );
        }
        content = content
            .child(
                div()
                    .id("content-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(div().p_6().pt_0().child(body)),
            )
            .child(
                div()
                    .px_6()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .flex()
                    .justify_between()
                    .gap_3()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.live.notice.clone())
                    .child(Button::new("theme").ghost().label("Switch theme").on_click(
                        |_, window, cx| {
                            let mode = if cx.theme().mode == ThemeMode::Dark {
                                ThemeMode::Light
                            } else {
                                ThemeMode::Dark
                            };
                            Theme::change(mode, Some(window), cx);
                        },
                    )),
            );
        div()
            .key_context("AuJitter")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_stretch()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &ToggleGaming, _, cx| this.toggle_gaming(cx)))
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| this.toggle_pause(cx)))
            .on_action(
                cx.listener(|this, _: &ExportEvidence, _, cx| this.send(Operation::Export, cx)),
            )
            .on_action(|_: &OpenWeb, _, cx| cx.open_url("http://127.0.0.1:9876"))
            .child(sidebar)
            .child(content)
    }
}

fn activate_native_window(window: &mut Window) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            IsWindowVisible, SW_SHOW, ShowWindowAsync,
        };
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            // This is our own live GPUI window, on its owning UI thread. A launcher's
            // hidden STARTUPINFO can override the first show; explicit Open must win.
            // Preserve the placement of windows that are already visible/minimized.
            unsafe {
                let hwnd = handle.hwnd.get() as _;
                if IsWindowVisible(hwnd) == 0 {
                    ShowWindowAsync(hwnd, SW_SHOW);
                    ShowWindowAsync(hwnd, SW_SHOW);
                }
            }
        }
    }
    window.activate_window();
}

fn show_window(session: Entity<DesktopState>, cx: &mut App) {
    tracing::debug!(windows = cx.windows().len(), "Showing AuJitter window");
    if let Some(handle) = cx.windows().into_iter().next() {
        let _ = handle.update(cx, |_, window, cx| {
            activate_native_window(window);
            cx.activate(true);
        });
        return;
    }
    // Fixed pixels here are the native window boundary, not component geometry.
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1180.0), px(800.0)),
            cx,
        ))),
        window_min_size: Some(size(px(900.0), px(600.0))),
        titlebar: Some(TitlebarOptions {
            title: Some("AuJitter".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let owner = session.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| NetworkWindow::new(owner, window, cx))
    }) {
        Ok((handle, _)) => {
            let _ = handle.update(cx, |_, window, cx| {
                activate_native_window(window);
                cx.activate(true);
            });
            tracing::info!("AuJitter window opened");
        }
        Err(error) => {
            tracing::error!(%error, "Could not open AuJitter window");
            session.update(cx, |session, cx| {
                session.live.error = Some(format!("Could not open AuJitter: {error}"));
                session.live.notice =
                    "Use Open web dashboard from the tray to view monitoring.".into();
                cx.notify();
            });
        }
    }
}

pub fn run() {
    let background = std::env::args().any(|arg| arg == "--background");
    let instance = match DesktopInstance::acquire(background) {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(error) => {
            eprintln!("Could not start AuJitter desktop: {error:#}");
            return;
        }
    };
    let request_path = instance.request_path();
    let tray_support = tray::check_support();
    let app = gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .with_quit_mode(QuitMode::Explicit);
    app.on_reopen(|cx| {
        let session = cx.global::<DesktopSession>().0.clone();
        show_window(session, cx);
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        let session = cx.new(|cx| DesktopState::new(request_path, tray_support, cx));
        cx.set_global(DesktopSession(session.clone()));
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-g"), ToggleGaming, Some("AuJitter")),
            KeyBinding::new(&format!("{modifier}-p"), TogglePause, Some("AuJitter")),
            KeyBinding::new(&format!("{modifier}-e"), ExportEvidence, Some("AuJitter")),
            KeyBinding::new(&format!("{modifier}-q"), Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| {
            let session = cx.global::<DesktopSession>().0.clone();
            session.update(cx, |session, cx| {
                session.handle_action(TrayAction::Quit, cx)
            });
        });
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() && cx.global::<DesktopSession>().0.read(cx).tray.is_none() {
                cx.quit();
            }
        })
        .detach();
        cx.on_app_quit(|cx| {
            let session = cx.global::<DesktopSession>().0.clone();
            session.update(cx, |session, _| {
                session.tray.take();
            });
            async {}
        })
        .detach();
        if !background || session.read(cx).tray.is_none() {
            show_window(session, cx);
        }
    });
    drop(instance);
}
