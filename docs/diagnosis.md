# Interpreting network evidence

AuJitter reports the likely **area** and **confidence**, with the probes that support it. It cannot determine contractual fault between the router vendor, access-network operator and retail ISP from end-to-end probes alone.

| Observed evidence | Likely area | What it does and does not establish |
| --- | --- | --- |
| No default interface; all TCP tests fail | Local link | Check Wi-Fi/Ethernet and OS routing. VPNs or unusual routing can complicate interface detection. |
| A previously responsive gateway fails alongside all TCP targets | Local network | Device, Wi-Fi/Ethernet and router remain possible. Compare another device and Ethernet. |
| Gateway works; all TCP targets fail | Upstream | The router's LAN is reachable; router WAN, mobile access, NBN and ISP remain possible. |
| Gateway works; all TCP targets fail; router reports WAN disconnected | Router WAN | Stronger evidence of WAN disconnection, still not proof of who caused it. |
| Gateway takes over 50 ms with internet still reachable | Local latency | Wi-Fi contention/device/router delay is plausible. Routers can deprioritize ICMP. |
| Configured DNS fails; independent DNS works; internet TCP works | DNS | Strong comparative evidence of a resolver/path issue. VPN policy and filtering still matter. |
| One internet target fails; another works | Target or route | Does not establish a general outage. |
| Previously responsive echo fails; TCP works | Echo loss or filtering | Echo loss is not the same as loss to the game server. |
| Internet echo latency/jitter exceeds thresholds | Latency | Congestion, radio conditions and routing are candidates. Compare gateway timings and other devices. |

For the reported League of Legends spikes, leave the monitor running and note the time of the lag. Compare the internet and gateway results in that interval. A matching gateway spike strengthens the local Wi-Fi/router hypothesis. A quiet gateway with delayed internet replies points farther upstream, but cannot separate router WAN congestion from mobile access or ISP routing without router telemetry or a second viewpoint.

Use gaming mode during a match; use normal mode for closer investigation outside a match. Probes use a bounded timeout: a reply taking 5,000 ms is normally recorded as a timeout, not as an invented RTT value. The target RTT is not the actual League server RTT. The app deliberately avoids reading or capturing game traffic.

Internet RTT is the latest timed ICMP reply. p95 uses successful replies in a rolling window of up to 120 samples and five minutes. Jitter is the mean absolute difference of consecutive successful internet RTTs; failures break a pair. Echo failure rate counts only supported attempts and is withheld when no reply has ever been seen in that window or there are fewer than three attempts. TCP failure rate is separate. Unstable-observation percentage counts recorded samples, not the percentage of every second of the day.

One unstable sample opens an incident. Three stable samples close it; unknown samples do not demonstrate recovery. Incidents keep the peak severity and corresponding diagnosis. A restart, pause, stop or observation gap closes a current record at the last observed sample, with a reason; later unobserved time is not billed as outage time. Intervals can miss short glitches. Missing ICMP support is recorded as unavailable.

Default IPv4 targets need IPv4 internet access. IPv6 unicast targets can be configured in the settings file; link-local IPv6 gateway/DNS scope and OS routing vary, so verify results on an IPv6-only network before relying on its diagnosis. VPNs, captive portals, firewalls and container NAT can alter the viewpoint. AuJitter does not disable them.
