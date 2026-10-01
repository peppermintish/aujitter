//! Native tray objects stay on GPUI's UI thread; callbacks only enqueue intent.
use super::{DesktopEvent, Live};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

/// Check Linux's actual tray host before creating a background-only desktop session.
/// Called before the GUI event loop starts, never from rendering or a menu callback.
pub(super) fn check_support() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let connection = zbus::blocking::connection::Builder::session()?
            .method_timeout(std::time::Duration::from_secs(2))
            .build()?;
        let watcher = zbus::blocking::Proxy::new(
            &connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )?;
        anyhow::ensure!(
            watcher.get_property::<bool>("IsStatusNotifierHostRegistered")?,
            "This desktop does not provide a system tray host"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(super) enum TrayAction {
    Open,
    Web,
    Pause,
    Gaming,
    Restart,
    Quit,
}

#[derive(PartialEq, Eq)]
struct Presentation {
    status: String,
    pause: &'static str,
    gaming: bool,
    controls_enabled: bool,
    restart_enabled: bool,
    quit_enabled: bool,
}

impl Presentation {
    fn new(live: &Live, quitting: bool, now: DateTime<Utc>) -> Self {
        let view = live.view.as_ref();
        let paused = view.is_some_and(|view| view.paused);
        let status = if quitting {
            "Stopping monitoring".into()
        } else if !live.connected {
            if live.error.is_some() {
                "Monitor unavailable".into()
            } else {
                "Starting monitor".into()
            }
        } else if paused {
            "Monitoring paused".into()
        } else if let Some(view) = view {
            match &view.latest {
                Some(sample)
                    if now.signed_duration_since(sample.at).num_milliseconds()
                        > (view.interval_ms.saturating_mul(3).max(60_000) as i64) =>
                {
                    "Waiting for new observations".into()
                }
                Some(sample) => format!("Monitoring · {}", sample.diagnosis.severity.label()),
                None => "Collecting evidence".into(),
            }
        } else {
            "Collecting evidence".into()
        };
        Self {
            status,
            pause: if paused {
                "&Resume monitoring"
            } else {
                "&Pause monitoring"
            },
            gaming: view.is_some_and(|view| view.gaming),
            controls_enabled: live.connected && !quitting,
            restart_enabled: !live.connected && !quitting,
            quit_enabled: !quitting,
        }
    }
}

pub(super) struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    pause: MenuItem,
    gaming: CheckMenuItem,
    restart: MenuItem,
    quit: MenuItem,
    presentation: Option<Presentation>,
}

impl Tray {
    pub(super) fn new(events: async_channel::Sender<DesktopEvent>) -> Result<Self> {
        let menu = Menu::new();
        let status = MenuItem::new("Starting monitor", false, None);
        let open = MenuItem::with_id("aujitter.open", "&Open AuJitter…", true, None);
        let web = MenuItem::with_id("aujitter.web", "Open &web dashboard…", true, None);
        let pause = MenuItem::with_id("aujitter.pause", "&Pause monitoring", false, None);
        let gaming = CheckMenuItem::with_id("aujitter.gaming", "Gaming &mode", false, false, None);
        let restart = MenuItem::with_id("aujitter.restart", "&Start monitor", true, None);
        let quit = MenuItem::with_id("aujitter.quit", "Stop monitoring and &quit", true, None);
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &open,
            &web,
            &PredefinedMenuItem::separator(),
            &pause,
            &gaming,
            &restart,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;
        let builder = TrayIconBuilder::new()
            .with_id("aujitter.tray")
            .with_menu(Box::new(menu))
            .with_tooltip("AuJitter · Starting monitor")
            .with_icon(Icon::from_rgba(
                include_bytes!("../../packaging/assets/tray32.rgba").to_vec(),
                32,
                32,
            )?)
            .with_menu_on_left_click(cfg!(target_os = "macos"));
        #[cfg(windows)]
        let builder = builder.with_guid(0xb1ee3ea5_3073_4793_a233_ce71af69ba8e);
        let icon = builder.build()?;
        let menu_events = events.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id().as_ref() {
                "aujitter.open" => TrayAction::Open,
                "aujitter.web" => TrayAction::Web,
                "aujitter.pause" => TrayAction::Pause,
                "aujitter.gaming" => TrayAction::Gaming,
                "aujitter.restart" => TrayAction::Restart,
                "aujitter.quit" => TrayAction::Quit,
                _ => return,
            };
            let _ = menu_events.try_send(DesktopEvent::Tray(action));
        }));
        TrayIconEvent::set_event_handler(Some(move |event| {
            // Native macOS/Linux menus own activation; Windows also supports left-click open.
            if cfg!(windows)
                && matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                )
            {
                let _ = events.try_send(DesktopEvent::Tray(TrayAction::Open));
            }
        }));
        Ok(Self {
            icon,
            status,
            pause,
            gaming,
            restart,
            quit,
            presentation: None,
        })
    }

    pub(super) fn update(&mut self, live: &Live, quitting: bool) {
        let presentation = Presentation::new(live, quitting, Utc::now());
        if self.presentation.as_ref() == Some(&presentation) {
            return;
        }
        self.status.set_text(&presentation.status);
        self.pause.set_text(presentation.pause);
        self.pause.set_enabled(presentation.controls_enabled);
        self.gaming.set_checked(presentation.gaming);
        self.gaming.set_enabled(presentation.controls_enabled);
        self.restart.set_enabled(presentation.restart_enabled);
        self.quit.set_enabled(presentation.quit_enabled);
        let _ = self
            .icon
            .set_tooltip(Some(format!("AuJitter · {}", presentation.status)));
        self.presentation = Some(presentation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Settings, monitor};

    #[test]
    fn unavailable_monitor_never_shows_a_cached_healthy_status() {
        let live = Live {
            view: Some(monitor::initial_dashboard(&Settings::default())),
            error: Some("Connection refused".into()),
            ..Default::default()
        };
        let display = Presentation::new(&live, false, Utc::now());
        assert_eq!(display.status, "Monitor unavailable");
        assert!(!display.controls_enabled);
        assert!(display.restart_enabled);
    }

    #[test]
    fn paused_monitor_offers_resume_and_preserves_gaming() {
        let mut view = monitor::initial_dashboard(&Settings::default());
        view.paused = true;
        view.gaming = true;
        let live = Live {
            connected: true,
            view: Some(view),
            ..Default::default()
        };
        let display = Presentation::new(&live, false, Utc::now());
        assert_eq!(display.status, "Monitoring paused");
        assert_eq!(display.pause, "&Resume monitoring");
        assert!(display.controls_enabled && display.gaming);
    }

    #[test]
    fn quitting_disables_controls_until_shutdown_finishes() {
        let live = Live {
            connected: true,
            ..Default::default()
        };
        let display = Presentation::new(&live, true, Utc::now());
        assert_eq!(display.status, "Stopping monitoring");
        assert!(!display.controls_enabled && !display.restart_enabled && !display.quit_enabled);
    }

    #[test]
    fn stale_samples_do_not_appear_as_current_network_health() {
        use crate::model::{Diagnosis, Metrics, Sample, Topology};
        let now = Utc::now();
        let mut view = monitor::initial_dashboard(&Settings::default());
        view.latest = Some(Sample {
            at: now - chrono::Duration::minutes(2),
            interval_ms: 5000,
            topology: Topology::default(),
            probes: vec![],
            metrics: Metrics::default(),
            diagnosis: Diagnosis::default(),
            observation_gap_seconds: None,
            monitoring_profile: None,
        });
        let live = Live {
            connected: true,
            view: Some(view),
            ..Default::default()
        };
        assert_eq!(
            Presentation::new(&live, false, now).status,
            "Waiting for new observations"
        );
    }
}
