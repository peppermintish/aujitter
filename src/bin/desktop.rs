#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,aujitter=info".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();
    aujitter::desktop::run();
}
