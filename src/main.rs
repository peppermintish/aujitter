use anyhow::{Context, Result, bail, ensure};
use aujitter::{
    config::{self, Settings},
    diagnosis::Analyzer,
    model::Dashboard,
    monitor,
    service::{self, ServiceOptions},
    store::Store,
    topology,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    fs::OpenOptions,
    io::BufWriter,
    net::SocketAddr,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
#[derive(Parser)]
#[command(
    version,
    about = "Quiet network monitoring with local history and evidence-based diagnosis"
)]
struct Cli {
    #[arg(long, global = true, env = "AUJITTER_CONFIG")]
    config: Option<PathBuf>,
    #[arg(long, global = true, env = "AUJITTER_DB")]
    database: Option<PathBuf>,
    #[arg(long, global = true, env = "AUJITTER_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// List situation presets (works even when monitoring is stopped).
    Presets,
    /// Apply a built-in preset to the running monitor without changing network or retention settings.
    Preset {
        #[arg(value_enum)]
        name: aujitter::presets::Preset,
        #[arg(long, default_value = "http://127.0.0.1:9876")]
        url: String,
    },
    /// Run the monitor and web dashboard; Ctrl+C stops gracefully.
    Run {
        #[arg(long, default_value = "127.0.0.1:9876")]
        bind: SocketAddr,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        samples: Option<u64>,
    },
    /// Start background monitoring which survives closing the window.
    Start,
    /// Opt in to or disable background monitoring at sign-in.
    Startup {
        #[arg(value_enum)]
        action: StartupAction,
    },
    /// Measure once without writing history.
    Once {
        #[arg(long)]
        json: bool,
    },
    /// Read the live monitoring status.
    Status {
        #[arg(long, default_value = "http://127.0.0.1:9876")]
        url: String,
        #[arg(long)]
        json: bool,
    },
    /// Read retained incidents even when monitoring is stopped.
    History {
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Export all retained history as streaming JSON Lines.
    Export {
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        days: Option<u32>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Generate a validated settings file.
    Init,
    /// Pause/resume or change the gaming interval.
    Control {
        #[arg(value_enum)]
        action: Action,
        #[arg(long, default_value = "http://127.0.0.1:9876")]
        url: String,
    },
    /// Stop the background monitor.
    Stop {
        #[arg(long, default_value = "http://127.0.0.1:9876")]
        url: String,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Action {
    Pause,
    Resume,
    GamingOn,
    GamingOff,
}
#[derive(Clone, Copy, ValueEnum)]
enum StartupAction {
    Enable,
    Disable,
    Status,
}
fn request(client: &reqwest::Client, token: Option<&str>, url: &str) -> reqwest::RequestBuilder {
    let request = client.get(url);
    if let Some(token) = token {
        request.bearer_auth(token)
    } else {
        request
    }
}
fn status(view: &Dashboard) {
    if view.paused {
        println!("Paused — the monitor is not sending probes");
    }
    if let Some(sample) = &view.latest {
        println!(
            "{} | {} | confidence: {}",
            sample.diagnosis.severity.label(),
            sample.diagnosis.area,
            sample.diagnosis.confidence
        );
        println!("{}", sample.diagnosis.explanation);
        println!(
            "Latency: {} | Jitter: {} | Echo failure: {}",
            sample
                .metrics
                .latency_ms
                .map(|v| format!("{v:.1} ms"))
                .unwrap_or_else(|| "unavailable".into()),
            sample
                .metrics
                .jitter_ms
                .map(|v| format!("{v:.1} ms"))
                .unwrap_or_else(|| "collecting".into()),
            sample
                .metrics
                .icmp_loss_percent
                .map(|v| format!("{v:.1}%"))
                .unwrap_or_else(|| "unconfirmed".into())
        );
        println!(
            "Observed at {} | interval {} seconds | gaming {}",
            sample.at,
            view.interval_ms / 1000,
            view.gaming
        );
    } else {
        println!("Collecting the first sample…");
    }
}
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(config::default_config);
    let database = cli.database.unwrap_or_else(config::default_database);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()?;
    match cli.command {
        Commands::Run { bind, samples } => {
            let mut options = ServiceOptions::default();
            options.config = config_path;
            options.database = database;
            options.bind = Some(bind);
            options.token = cli.token;
            options.max_samples = samples;
            service::serve(options).await?;
        }
        Commands::Start => {
            if let Ok(reply) = client.get("http://127.0.0.1:9876/api/health").send().await
                && reply.status().is_success()
                && reply
                    .json::<serde_json::Value>()
                    .await?
                    .get("app")
                    .and_then(|s| s.as_str())
                    == Some("aujitter")
            {
                println!("AuJitter is already running at http://127.0.0.1:9876");
                return Ok(());
            }
            std::fs::create_dir_all(config::data_dir())?;
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(config::data_dir().join("monitor.log"))?;
            let mut child = Command::new(std::env::current_exe()?);
            child
                .arg("--config")
                .arg(&config_path)
                .arg("--database")
                .arg(&database)
                .arg("run")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log);
            if let Some(token) = cli.token {
                child.env("AUJITTER_TOKEN", token);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                child.creation_flags(0x08000000 | 0x00000200);
            }
            let mut process = child
                .spawn()
                .context("Could not start background monitoring")?;
            for _ in 0..30 {
                if let Some(code) = process.try_wait()? {
                    bail!("Monitor exited with {code}; inspect the monitor log");
                }
                if let Ok(reply) = client.get("http://127.0.0.1:9876/api/health").send().await
                    && reply.status().is_success()
                {
                    println!("Background monitoring started at http://127.0.0.1:9876");
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            bail!("Monitor launched, but startup is not confirmed; inspect the monitor log");
        }
        Commands::Presets => {
            for item in aujitter::presets::catalogue() {
                let id = serde_json::to_value(item.id)?
                    .as_str()
                    .unwrap()
                    .replace('_', "-");
                println!(
                    "{id}: {}\n  {}\n  Effective {}s; timeout {}ms; latency warning {}ms; jitter warning {}ms",
                    item.name,
                    item.description,
                    if item.gaming {
                        item.gaming_interval_ms
                    } else {
                        item.interval_ms
                    } / 1000,
                    item.timeout_ms,
                    item.latency_warning_ms,
                    item.jitter_warning_ms
                );
            }
        }
        Commands::Preset { name, url } => {
            ensure!(
                name != aujitter::presets::Preset::Custom,
                "Choose a built-in preset; custom preferences are edited in Settings"
            );
            let mut request = client
                .post(format!("{}/api/preset", url.trim_end_matches('/')))
                .json(&serde_json::json!({"preset":name}));
            if let Some(token) = &cli.token {
                request = request.bearer_auth(token);
            }
            request.send().await?.error_for_status()?;
            println!("Applied {} preset", name.name());
        }
        Commands::Startup { action } => {
            if matches!(action, StartupAction::Status) {
                println!("Start at sign-in: {}", aujitter::startup::is_enabled());
            } else {
                let desktop = std::env::current_exe()?.with_file_name(if cfg!(windows) {
                    "aujitter-gui.exe"
                } else {
                    "aujitter-gui"
                });
                aujitter::startup::set_enabled(matches!(action, StartupAction::Enable), desktop)?;
                println!("Start at sign-in preference saved");
            }
        }
        Commands::Once { json } => {
            let settings = Settings::load(&config_path)?;
            let sample = monitor::collect(
                &settings,
                topology::detect(&settings),
                &mut Analyzer::default(),
            )
            .await;
            if json {
                println!("{}", serde_json::to_string_pretty(&sample)?);
            } else {
                let mut view = monitor::initial_dashboard(&settings);
                view.latest = Some(sample);
                status(&view);
            }
        }
        Commands::Status { url, json } => {
            let view: Dashboard = request(
                &client,
                cli.token.as_deref(),
                &format!("{}/api/dashboard", url.trim_end_matches('/')),
            )
            .send()
            .await
            .context("Monitor is not reachable; run `aujitter start` first")?
            .error_for_status()?
            .json()
            .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                status(&view);
            }
        }
        Commands::History { limit, json } => {
            ensure!(database.exists(), "No monitoring history yet");
            let incidents = Store::open(&database)?.incidents(limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&incidents)?);
            } else {
                for row in incidents {
                    println!(
                        "#{} {} → {} | {} | {} | {} unstable samples",
                        row.id,
                        row.started_at,
                        row.ended_at
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "ongoing".into()),
                        row.peak_severity.label(),
                        row.diagnosis.area,
                        row.unstable_samples
                    );
                }
            }
        }
        Commands::Export {
            output,
            days,
            overwrite,
        } => {
            ensure!(database.exists(), "No monitoring history to export");
            let file = OpenOptions::new()
                .write(true)
                .create(overwrite)
                .create_new(!overwrite)
                .truncate(overwrite)
                .open(&output)
                .context("Could not create export; choose a new path or use --overwrite")?;
            let mut writer = BufWriter::new(file);
            Store::open(&database)?.export(
                &mut writer,
                days.map(|d| chrono::Utc::now() - chrono::Duration::days(d as i64)),
            )?;
            std::io::Write::flush(&mut writer)?;
            println!("Saved {}", output.display());
        }
        Commands::Init => {
            ensure!(
                !config_path.exists(),
                "Settings already exist at {}",
                config_path.display()
            );
            Settings::default().save(&config_path)?;
            println!("Saved {}", config_path.display());
        }
        Commands::Control { action, url } => {
            let payload = match action {
                Action::Pause => serde_json::json!({"paused":true}),
                Action::Resume => serde_json::json!({"paused":false}),
                Action::GamingOn => serde_json::json!({"gaming":true}),
                Action::GamingOff => serde_json::json!({"gaming":false}),
            };
            let mut request = client
                .post(format!("{}/api/control", url.trim_end_matches('/')))
                .json(&payload);
            if let Some(token) = &cli.token {
                request = request.bearer_auth(token);
            }
            request.send().await?.error_for_status()?;
            println!("Monitoring setting saved");
        }
        Commands::Stop { url } => {
            let mut request = client.post(format!("{}/api/shutdown", url.trim_end_matches('/')));
            if let Some(token) = &cli.token {
                request = request.bearer_auth(token);
            }
            request.send().await?.error_for_status()?;
            println!("Monitor is stopping");
        }
    }
    Ok(())
}
