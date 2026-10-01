AuJitter 0.1.0 provides a shared background monitor with CLI, local web dashboard and native GPUI Kit interface. It records observed interruptions, latency/jitter, DNS and TCP evidence, confidence-qualified diagnoses, and local history with manual export. Gaming mode reduces probe frequency; sign-in startup is optional.

Packages are provided for Windows, macOS and Linux on x64 and ARM64. Container images support Linux AMD64 and ARM64. Windows MSIX files use the reserved Store identity and are unsigned for future Store ingestion. Portable Windows ZIPs are usable without MSIX installation. macOS builds are ad-hoc signed, without Developer ID notarization. Linux builds require glibc 2.39 and a graphics environment for the GUI.

WAN technology detection depends on optional read-only router information. A specific router/ISP cause cannot always be proven from end-to-end probes. See the diagnosis, privacy, container and packaging guides included with these files.
