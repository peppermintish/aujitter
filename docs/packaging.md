# Native packages and Microsoft Store

The build workflow tests the core, compiles both executables and makes packages on six native GitHub-hosted runners. There is no x64 binary relabelled as ARM64.

| OS | x64 runner / target | ARM64 runner / target | Outputs |
| --- | --- | --- | --- |
| Windows | windows-2025 / x86_64-pc-windows-msvc | windows-11-arm / aarch64-pc-windows-msvc | ZIP, Store-associated MSIX, SHA256 |
| macOS | macos-15-intel / x86_64-apple-darwin | macos-15 / aarch64-apple-darwin | App ZIP, DMG, SHA256 |
| Linux | ubuntu-24.04 / x86_64-unknown-linux-gnu | ubuntu-24.04-arm / aarch64-unknown-linux-gnu | DEB, tar.gz, SHA256 |

Every push and pull request runs validation; successful package jobs upload downloadable artifacts for 30 days. A `v<version>` tag matching Cargo.toml publishes the same files as a GitHub release only after every package job succeeds. CI publishes containers in a separate workflow, smoke-tests both architectures and combines their manifests. Workflow permissions are scoped to read source, write packages, or publish a release as needed.

## Current Store identity

The product was reserved in the existing Partner Center account on 1 October 2026. Submission 1 was created and submitted for certification on 5 October 2026. Partner Center reports **In certification**, with pre-processing in progress and publication set to start automatically after certification passes. Both version 0.1.1.0 MSIX packages (x64 and ARM64) were uploaded, validated and included in the submission. The saved age rating is Microsoft Store/PEGI/IARC **3+** and ESRB **Everyone**. Microsoft still needs to complete certification, including review of the `runFullTrust` capability; package acceptance validation is not a certification pass.

User-visible branding is **AuJitter**. Microsoft's assigned identity keeps its original casing:

- Package Name: `Gofor.auJitter`
- Publisher: `CN=A687626B-CDF9-44E4-8D21-944561AEECD8`
- PublisherDisplayName: `Gofor`
- Package family: `Gofor.auJitter_efaz5cy249mmj`
- Store ID: `9NDKBXRLSXDT`
- [Partner Center product](https://partner.microsoft.com/en-us/dashboard/products/9NDKBXRLSXDT/overview)

These public package identity values live in `packaging/windows/identity.json`. The manifest uses them exactly, plus the actual architecture and a four-part package version (`0.1.1.0`). The fourth version part stays zero for Store submissions. It provides the desktop full-trust application, icons, and the `aujitter.exe` execution alias. Windows targets use a static C runtime. Monitor traffic is the app's own connectivity checks, without packet interception.

`MakeAppx pack` validates the manifest and creates an **unsigned** package. Microsoft Store ingestion can accept an unsigned MSIX and applies Store signing. Direct sideloading needs a valid signature trusted by that machine. The portable ZIP is immediately usable without installing a signing certificate. No self-signed trust root, certificate, or private signing key is installed automatically.

For future updates, select the corresponding MSIX files for x64 and ARM64 under this product. Finish the listing, screenshots, privacy-policy URL, support details, age rating, pricing/availability and any full-trust capability explanation, then run Windows App Certification Kit and review the package before submission. Local WACK testing has not completed: launching the installed tool required administrator approval. No local WACK pass is claimed. Reserving a name or passing MakeAppx is not Store certification. Partner Center currently asks for a submission within three months of name reservation.

The sign-in preference uses a user-level startup entry and an installed executable path. Moving a portable app requires refreshing that preference; packaged app updates can also change the path, so re-enable it after an update until a Store-specific StartupTask implementation is introduced. No system service is installed. To remove the entry before uninstalling, use Settings or `aujitter startup disable`.

## Distribution signing

Windows portable executables and initial macOS builds are not backed by purchased distribution certificates. The macOS app is ad-hoc signed to preserve ARM64 validity; it is not notarized. Set up certificate secrets and the appropriate platform signing/notarization workflow before public production distribution. The current workflows do not pretend that missing credentials produce a trusted signature, and do not store secrets in source or artifacts.

## Reproduce a package

```sh
cargo build --locked --release --features desktop --bins
python3 scripts/package.py --platform windows --arch x64
# Replace platform with macos or linux on that native OS.
# When cargo uses --target, give the same triple to package.py --target.
```

`scripts/create-icons.py` reproduces installer PNGs from the pulse design and requires Pillow; all needed images are already committed, so CI does not need Pillow. Linux DEBs target glibc 2.39 or newer and declare the GUI runtime libraries. A GPUI app needs a usable graphics session; a successful headless backend test does not validate every GPU or desktop environment.
