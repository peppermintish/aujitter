# Situation presets

All three interfaces use the same catalogue. Apply a preset in Settings, or list/apply one with `aujitter presets` and `aujitter preset <id>`. It takes effect on the next cycle. A preset adjusts the normal/gaming intervals, timeout, latency/jitter warnings and gaming-mode choice. It preserves pause state, ISP and WAN labels, gateway/DNS overrides, targets, retention, sign-in startup and router read permission. Presets never enable router discovery or change the router, OS DNS, Wi-Fi, VPN or ISP configuration.

| CLI id | Situation | Effective interval | Timeout | Latency / jitter warning |
| --- | --- | --- | --- | --- |
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

The separate gaming switch always selects that preset's slower gaming interval: 15 s for everyday/calls/Wi-Fi/dropout, 30 s for streaming/mobile/satellite/VPN/server, and 120 s for quiet. Gaming mode skips optional router discovery and status reads. Applying any other preset turns gaming mode off; the UI shows the resulting interval. Investigative profiles are intended for use outside a match.

These are sensible starting points, not coverage of every possible connection or application. Satellite services vary substantially; tune the threshold to the actual service. VPN probes follow the host's routing and configured DNS, and do not bypass corporate policy. A streaming preset cannot measure bitrate or prove buffering. A call preset cannot inspect call quality. The game preset does not inspect a game or measure its server directly.

All modes record the same evidence and use the same diagnosis rules. A slow local gateway still warns above 50 ms. Wider intervals reduce traffic but miss shorter events. Timeouts create recorded failures rather than fabricated RTTs. Manual timing/warning edits are labelled Custom while retaining the latest settings. Switching situations never silently resumes a paused monitor.
