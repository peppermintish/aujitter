# AuJitter

Quiet network monitoring for intermittent home internet. AuJitter records latency, interruptions, DNS failures and the evidence needed to distinguish a local network problem from an upstream problem. It has a native [GPUI Kit](https://github.com/longbridge/gpui-kit) desktop window, a CLI, and a local web dashboard sharing one background monitor and SQLite history.

Designed for Wi-Fi and Ethernet on Australian connections: TPG, Telstra, Optus, Aussie Broadband, Superloop and other providers, with mobile, NBN, fibre, HFC/cable, DSL, fixed wireless and satellite access. Probes are provider independent. Identifying a router's active WAN technology needs information the router actually exposes; unknown stays unknown.

## Run it

Download a package for your OS and architecture from [GitHub Actions](https://github.com/peppermintish/aujitter/actions/workflows/build.yml), or a tagged [release](https://github.com/peppermintish/aujitter/releases). x64 and AMD64 mean the same architecture. ARM64 is a separate build.

- **Windows:** extract the ZIP and launch `aujitter-gui.exe`. Keep `aujitter.exe` beside it. CI also produces an unsigned MSIX associated with the reserved Microsoft Store product; see [Store packaging](docs/packaging.md).
- **macOS:** open the DMG and copy AuJitter into Applications. Initial builds are ad-hoc signed and are not yet Developer ID signed or notarized. The CLI is inside `AuJitter.app/Contents/MacOS/aujitter`.
- **Linux:** install the `.deb` on Ubuntu 24.04 or newer, or unpack the portable archive with the required native libraries installed. The GUI needs a working Vulkan graphics driver and an X11 or Wayland desktop.

Opening the desktop starts the monitor. Closing the desktop or browser leaves it running. Enable **Start at sign-in** in desktop Settings to opt in. The CLI offers the same preference for installations that include both binaries.

```sh
aujitter start
aujitter status
aujitter control gaming-on
aujitter control pause
aujitter control resume
aujitter startup enable
aujitter history --limit 100
aujitter export --output evidence.jsonl --days 7
aujitter stop
```

The browser dashboard is at **http://127.0.0.1:9876**. CLI `run` stays in the foreground; `once --json` checks connectivity without saving history. `startup disable` removes only AuJitter's own sign-in entry. After moving a portable installation, disable and enable its startup preference again.

## What it measures

Every 5 seconds by default, the monitor sends one echo to the gateway, one to an internet target, makes small TCP connection checks to two independent targets on port 443, and sends one DNS query to the configured resolver. A second DNS resolver is consulted only if the first fails. Probes have a 1-second timeout and run concurrently; slow probes never create a catch-up burst.

**Gaming mode** increases the interval to 15 seconds and skips optional router discovery and router reads. There are no speed tests, large downloads, packet capture, game hooks, continuous animations, or automatic traceroutes. Some traffic and CPU are unavoidable, so zero gaming impact cannot be guaranteed. Pause sends no measurement probes. GUI/web refreshes are local and limited to 5 seconds.

The monitor records every **observed** unstable sample, even a one-sample incident. A paced monitor cannot capture a glitch shorter than the interval or an event while the computer is asleep. Sleep, restart, clock changes and pauses are explicitly distinguished from a measured outage. Three healthy samples close an incident.

| Status | Meaning |
| --- | --- |
| Collecting baseline | Not enough comparable observations yet, or a probe is unavailable |
| Stable | Current reachability works and configured latency thresholds are satisfied |
| Watch | A target/route or previously responsive echo failed while other evidence still works |
| Degraded | Slow local replies, excessive internet latency/jitter, or DNS failure |
| Offline | Every independent internet TCP target failed |

Latency defaults to a 150 ms warning, rolling jitter to 30 ms. Local gateway latency over 50 ms provides a separate local-network warning. Internet echo failures are displayed as **loss or filtering**, and a router that never answers ICMP is not automatically blamed. Details and limitations: [diagnosis guide](docs/diagnosis.md).

## History and privacy

Default retention is **30 days of detailed samples**, **365 days of incidents and hourly summaries**, stored on this device. Settings and history live in the OS user data directory. CLI options `--config` and `--database`, or `AUJITTER_CONFIG` and `AUJITTER_DB`, allow custom locations. The native interface uses the default location or those environment variables.

The application has no analytics, account requirement, cloud history, or automatic uploads. Default probes contact Cloudflare (`1.1.1.1`) and Google (`8.8.8.8`); the DNS test asks for `example.com`. This is connectivity traffic visible to its destinations. It does not inspect browsing, DNS history, or game traffic. Read [privacy details](docs/privacy.md) before sharing an export.

GUI/web **Export evidence** saves a report explicitly limited to the last 120 samples, 100 incidents and 24 hourly summaries. CLI `export` streams the full retained history, optionally restricted by `--days`, and refuses to overwrite a file unless `--overwrite` is supplied. The database uses bounded transactions and automatic hourly retention pruning.

## Router identification

The active interface, local transport, gateway and DNS are detected automatically. Wi-Fi's **5 GHz band is not cellular 5G**. Optional router discovery reads UPnP IGD model and WAN information, if your router already exposes it. The only router actions are `GetStatusInfo` and `GetCommonLinkProperties`. There are no router setting changes or credentials stored.

UPnP often exposes cable or DSL, but not precise NBN technology or a mobile modem's active backhaul and radio signal. A 5G-capable model is only evidence of capability. You can set a known connection type and ISP label manually. A model-specific, read-only radio integration can be added once the exact router model and documented API are known. Don't enable router services merely to use the core monitor.

## Containers

For an existing Docker environment, follow the [Compose guide](docs/docker.md). CI publishes `ghcr.io/peppermintish/aujitter` for Linux AMD64 and ARM64. Containers provide the CLI and browser dashboard; the native desktop is packaged separately. A bridged container sees its container gateway, and Docker Desktop adds a VM, so use the native monitor for reliable Wi-Fi versus router diagnosis.

## Build and develop

Rust is pinned to **1.98.1** and GPUI Kit to **0.7.0**, the current Kit release checked on 1 October 2026. `Cargo.lock` is committed. A normal backend build does not pull native GUI code into its executable.

```sh
cargo test --locked --no-default-features
cargo run --locked -- once
cargo run --locked -- run
cargo build --locked --features desktop --bins
cargo run --locked --features desktop --bin aujitter-gui
cargo fmt --all --check
cargo clippy --locked --features desktop --all-targets -- -D warnings
```

Linux GUI build dependencies are listed in `.github/workflows/build.yml`. Native packages use `scripts/package.py` after a release build; Windows requires the Windows SDK's MakeAppx, macOS uses `hdiutil`, and Linux uses `dpkg-deb`. `scripts/smoke.py <path-to-aujitter>` exercises the running service with monitoring paused and no external measurement traffic.

The design was built using **[UI UX Pro Max](https://github.com/nextlevelbuilder/ui-ux-pro-max-skill)** and GPUI Kit's own agent skills. Rationale and native adaptations are recorded in [design notes](docs/design.md). Project architecture: [architecture notes](docs/architecture.md). Licensed under [MIT](LICENSE).
