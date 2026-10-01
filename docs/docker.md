# Quick start with Docker Compose

Requires Docker Engine and Compose v2. The image contains the CLI and web dashboard for Linux AMD64 and ARM64; Docker chooses the architecture. The GUI is distributed in native OS packages.

1. Download `compose.yaml` and `.env.example` from this repository into one directory.
2. Copy `.env.example` to `.env`. Set `AUJITTER_TOKEN` to a fresh random value of at least 24 characters. For example, `openssl rand -hex 32` produces a suitable value. Keep this file private and out of source control.
3. Start and check the monitor:

```sh
docker compose pull
docker compose up -d
docker compose ps
docker compose logs --tail 50
```

Open **http://127.0.0.1:9876**, enter the token and choose Connect. The token is kept for that browser tab's session. Monitoring history stays in the `aujitter-data` named volume at `/data/history.sqlite3`, with settings at `/data/settings.json`. The container runs as user 10001, uses a read-only root filesystem, drops capabilities and permits unprivileged ICMP through `ping_group_range`. It does not need privileged mode.

```sh
docker compose exec aujitter aujitter status
docker compose exec aujitter aujitter control gaming-on
docker compose exec aujitter aujitter history --json
docker compose exec aujitter aujitter export --output /data/evidence.jsonl
docker compose cp aujitter:/data/evidence.jsonl ./evidence.jsonl
docker compose down
```

The token environment variable also authenticates these CLI requests. `down` preserves the named volume. `down -v` deletes the history and settings; only use it when you intend to remove those records. Stop/restart receives SIGTERM and closes incident records gracefully. Default limits are 256 MiB RAM, 0.25 CPU and 64 processes; adjust them if your host needs different limits.

The image removes ping's installed file capability and uses the container's unprivileged ping sockets. This avoids a Linux execution denial when all container capabilities are dropped; see the [Linux capabilities manual](https://man7.org/linux/man-pages/man7/capabilities.7.html). CI checks the same restrictions and a loopback echo before publishing either architecture. TCP and DNS evidence remains available on hosts that restrict ICMP.

## Network viewpoint

In the default bridge configuration, the gateway belongs to the Docker network. Docker Desktop on Windows/macOS adds a Linux VM. It cannot reliably decide whether a Wi-Fi spike came from the real router, and container interface types do not identify your physical connection. Use the native AuJitter monitor on the gaming PC for that investigation.

For a **Linux Docker Engine host**, use the optional host-network override with Compose 2.24.4 or newer (it uses `!reset`):

```sh
docker compose -f compose.yaml -f compose.host.yaml up -d
```

This reads the Linux host's network route and binds the dashboard to localhost. Some hosts disallow unprivileged ICMP; check `/proc/sys/net/ipv4/ping_group_range` and the recorded probe state. TCP/DNS evidence still works when ICMP is unavailable. The host override does not change the host's kernel settings. Do not use it as a way to diagnose physical adapters through Docker Desktop's VM.

## Updates and image tags

`latest` follows the repository's default branch or a tagged release after both architecture builds pass. `sha-<full-commit>` is an immutable version choice in the workflow, and version tags such as `0.1.0` are produced on matching Git tags. Each published manifest includes AMD64 and ARM64. Pin a reviewed digest or version for a long-term deployment.

```sh
docker compose pull
docker compose up -d
```

GHCR package visibility must be public to permit anonymous pulls. If the owner keeps it private, authenticate with a GitHub account that has package read access using the normal Docker login flow. CI uses the repository's short-lived `GITHUB_TOKEN`; no personal token is committed. To build locally, use `docker build -t aujitter:local .` and replace the Compose image accordingly.
