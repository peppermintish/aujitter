# Situation presets

All three interfaces use the same catalogue. Apply a preset in Settings, or list/apply one with `aujitter presets` and `aujitter preset <id>`. It takes effect on the next cycle. A preset adjusts the normal/gaming intervals, timeout, latency/jitter warnings and gaming-mode choice. It preserves pause state, ISP and WAN labels, gateway/DNS overrides, targets, retention, sign-in startup and router read permission. Presets never enable router discovery or change the router, OS DNS, Wi-Fi, VPN or ISP configuration.

| CLI id | Situation | Effective interval | Timeout | Latency / jitter warning |
| --- | --- | --- | --- | --- |
| automatic | Choose from connection evidence | 5–10 s; 15–30 s with gaming on | From chosen preset | From chosen preset |
| everyday | Most home connections | 5 s | 1 s | 150 / 30 ms |
| gaming | Playing a game | 15 s, gaming on | 1 s | 150 / 30 ms |
| calls | Video calls / work | 5 s | 1 s | 100 / 20 ms |
| streaming | Reachability during streaming | 10 s | 1 s | 250 / 60 ms |
| wifi-check | Compare Wi-Fi with Ethernet | 3 s | 1 s | 100 / 20 ms |
| dropout-hunt | Closer observation of interruptions | 3 s | 2 s | 150 / 30 ms |
| mobile | 5G / 4G internet | 10 s | 1.5 s | 200 / 60 ms |
| satellite | Higher expected satellite delay | 10 s | 2 s | 900 / 120 ms |
| quiet | Limited data / low activity | 60 s | 1 s | 200 / 60 ms |
| vpn | VPN / work network | 10 s | 1.5 s | 250 / 60 ms |
| server | Unattended host | 10 s | 1 s | 150 / 30 ms |

The separate gaming switch always selects that preset's slower gaming interval: 15 s for everyday/calls/Wi-Fi/dropout, 30 s for streaming/mobile/satellite/VPN/server, and 120 s for quiet. Gaming mode skips optional router discovery and status reads. Applying Automatic preserves your gaming choice; applying another non-gaming preset turns gaming mode off. The UI shows the resulting interval. Investigative profiles are intended for use outside a match.

## Automatic selection

New installations default to Automatic. Existing explicitly selected presets stay selected. `aujitter preset automatic` enables it on a running monitor. The default-route adapter is checked at most every 30 seconds; no application names, game processes, packet contents or browser activity are inspected.

Selection follows this order:

1. An OS-reported tunnel, a numbered `tun`/`tap`/`utun`/`wg` interface, or a recognised VPN adapter name selects VPN / work timing. This is an adapter hint, not proof of a corporate network or VPN policy. Split tunnels can leave the ordinary adapter as the default route and cannot always be recognised.
2. A configured or reported satellite WAN selects Satellite timing.
3. An OS-reported mobile broadband adapter, or configured/reported 4G, 5G or fixed wireless WAN selects Mobile timing. A mobile adapter alone does not prove whether its radio is currently using 4G or 5G.
4. Other or unknown connections use Everyday timing. Wi-Fi and Ethernet identify the PC's local link; they cannot identify a router's WAN. A 5G-capable router name alone never changes the WAN type or preset.

Known WAN labels you enter take precedence over router metadata. Router metadata is used only with your read-only discovery permission enabled. The current generic integration can report cable/DSL; mobile radio and precise NBN subtype detection may require a future model-specific integration. Disabling discovery stops using its cached WAN type. Containers default to Everyday unless there is connection evidence or an explicit WAN label; they do not infer the host's Wi-Fi from a Docker adapter.

The dashboard, native GUI and CLI show both **Automatic** and its effective choice, with a reason. Each new stored sample includes the effective preset, timeout and warning thresholds so later diagnosis can be interpreted correctly. Automatic choices stay in memory; your saved preference remains Automatic rather than being overwritten every time the route changes. Old samples have no profile metadata and still load normally.

Automatic never starts an investigative 3-second profile or tries to recognise calls, streaming or games. Use the gaming switch during play; use a manual preset when you want a particular investigation. Manual timing/warning edits select Custom, while changing a WAN label or router permission leaves Automatic enabled.

These are sensible starting points, not coverage of every possible connection or application. Satellite services vary substantially; tune the threshold to the actual service. VPN probes follow the host's routing and configured DNS, and do not bypass corporate policy. A streaming preset cannot measure bitrate or prove buffering. A call preset cannot inspect call quality. The game preset does not inspect a game or measure its server directly.

All modes record the same evidence and use the same diagnosis rules. A slow local gateway still warns above 50 ms. Wider intervals reduce traffic but miss shorter events. Timeouts create recorded failures rather than fabricated RTTs. Manual timing/warning edits are labelled Custom while retaining the latest settings. Switching situations never silently resumes a paused monitor.
