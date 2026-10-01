# Privacy policy

AuJitter 0.1.0 stores monitoring settings and history on the device running its monitor. It does not upload history, collect product analytics, or require an account. GitHub is used to distribute source, packages and container images; Microsoft Store is an optional distribution channel.

Stored samples can contain local interface names, local IP addresses, gateway and DNS addresses, configured ISP labels, router model/WAN status, probe targets, timings and errors. The default retention is 30 days of detailed samples and 365 days of incidents and hourly summaries. Changing retention to zero keeps that category indefinitely. Pruning deletes expired rows; SQLite may retain free pages for reuse, so this is not a forensic secure-erasure tool.

Connectivity tests send minimal ICMP/TCP traffic to configured targets and a DNS query for `example.com` to a configured resolver. Defaults are Cloudflare and Google public IPs. Those destinations and intervening networks can observe the connection's public address and traffic. No browser history, packet payloads, game traffic or passwords are read. Optional discovery uses LAN SSDP and reads UPnP IGD metadata and two read-only actions from the gateway. It is off by default.

Automatic preset selection uses the default-route adapter's type/name and available WAN evidence. It does not read running application names or processes. Its choice, reason, timeout and warning thresholds are recorded with new samples; it does not contact an ISP-identification service.

Manual export writes an evidence file locally; it does not send it to an ISP or support service. Review local addresses and labels before sharing. Web/native export is a clearly described recent snapshot; CLI export can include every retained record. You control copying and sending these files.

The desktop service listens only on 127.0.0.1 by default. A non-loopback listener requires an access token of at least 24 characters. The supplied Compose setup publishes only to localhost and requires a token. Do not expose plain HTTP and its token to the public internet; use a private trusted network or your own HTTPS reverse proxy. Browser tokens are held in session storage for that tab, not saved in settings or history. Tokens passed through environment variables must be protected by the container host.

To remove data, stop the monitor, disable optional sign-in startup, and remove the selected configuration/database/export files or Docker named volume. Uninstalling the application alone may preserve user data. Questions or problems can be reported through the repository's issue tracker without attaching private logs.
