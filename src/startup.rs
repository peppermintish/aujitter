use anyhow::{Result, ensure};
use std::path::PathBuf;

#[cfg(windows)]
pub fn is_enabled() -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "AuJitter",
        ])
        .creation_flags(0x08000000)
        .output()
        .is_ok_and(|r| r.status.success())
}
#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    startup_file().exists()
}

#[cfg(not(windows))]
fn startup_file() -> PathBuf {
    let base = directories::BaseDirs::new().expect("User directories unavailable");
    #[cfg(target_os = "macos")]
    {
        base.home_dir()
            .join("Library/LaunchAgents/au.aujitter.monitor.plist")
    }
    #[cfg(not(target_os = "macos"))]
    {
        base.config_dir().join("autostart/aujitter.desktop")
    }
}

/// Opt-in user-level startup; never writes system services or requires admin access.
pub fn set_enabled(enabled: bool, executable: PathBuf) -> Result<()> {
    let path = executable.to_string_lossy();
    ensure!(
        executable.is_absolute() && executable.exists(),
        "Start at sign-in requires an installed desktop executable"
    );
    ensure!(
        !path.contains(['\n', '\r', '"']),
        "Executable path contains unsupported characters"
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("reg");
        if enabled {
            command.args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "AuJitter",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{path}\" --background"),
                "/f",
            ]);
        } else {
            if !is_enabled() {
                return Ok(());
            }
            command.args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "AuJitter",
                "/f",
            ]);
        }
        ensure!(
            command
                .creation_flags(0x08000000)
                .output()?
                .status
                .success(),
            "Could not update start at sign-in"
        );
    }
    #[cfg(not(windows))]
    {
        let file = startup_file();
        if enabled {
            std::fs::create_dir_all(file.parent().unwrap())?;
            #[cfg(target_os = "macos")]
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>au.aujitter.monitor</string><key>ProgramArguments</key><array><string>{}</string><string>--background</string></array><key>RunAtLoad</key><true/></dict></plist>",
                path.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            );
            #[cfg(not(target_os = "macos"))]
            let content = format!(
                "[Desktop Entry]\nType=Application\nName=AuJitter monitor\nExec=\"{}\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
                path.replace('\\', "\\\\")
                    .replace('%', "%%")
                    .replace('`', "\\`")
                    .replace('$', "\\$")
            );
            std::fs::write(file, content)?;
        } else if file.exists() {
            std::fs::remove_file(file)?;
        }
    }
    Ok(())
}
