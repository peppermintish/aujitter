# Microsoft Store updates from GitHub

Stable version tags run the native checks/builds, publish the GitHub release, and then call `.github/workflows/store.yml` directly. The Store job downloads that release's Windows x64 and ARM64 MSIX files and checksums, verifies their Store identity, version and executable architectures, and makes one validated MSIX bundle containing both architectures. It uses Microsoft's App Publisher action v1.4 (pinned by commit) and Store CLI v0.4.3 to upload the bundle and submit it for certification. AuJitter remains free; the CLI's base price is explicitly `Free` to support Partner Center's newer pricing representation.

Publication still requires Microsoft's certification. Successful upload/submission is not a certification pass. The first version must finish certification and go live before automated updates can run.

## Credentials

Set these four **GitHub Actions repository secrets** on `peppermintish/aujitter`:

| Secret | Value |
| --- | --- |
| `AZURE_AD_TENANT_ID` | GUID of the Microsoft Entra tenant associated with this Partner Center account |
| `AZURE_AD_APPLICATION_CLIENT_ID` | GUID of the dedicated AuJitter CI application registration |
| `AZURE_AD_APPLICATION_SECRET` | Client secret **value**, not the secret's ID |
| `SELLER_ID` | Numeric Partner Center developer seller ID, not the CN publisher ID |

The Entra application must be added to Partner Center with permission to manage Store submissions. Restrict access to AuJitter where the account's role controls allow it. Creating that identity, granting access and generating its credential require the account owner's approval. Keep a record of the client secret's expiration and replace it in GitHub before expiry. GitHub does not return saved secret values, so a secrets list only proves presence; the read-only Store API check proves authentication and access to AuJitter.

The Store product ID, package identity, publisher and package family are public identifiers in `packaging/windows/identity.json`. The release checks compare both the packages and the authenticated Store API response with that file. They are not client credentials and do not need separate GitHub secrets.

Only the four named secrets reach the Store job. They are passed through environment variables, never interpolated into shell code. The script captures credential-bearing commands without printing their arguments or raw responses, disables CLI telemetry, and the workflow resets its temporary CLI credentials on exit. Uploaded CI artifacts contain packages and package-validation metadata only.

## Validate without changing the Store

Open [Microsoft Store release](https://github.com/peppermintish/aujitter/actions/workflows/store.yml), select **Run workflow**, enter an existing stable GitHub release tag and leave **publish unchecked**. The package job validates/bundles actual release files. The Store job verifies secret presence/format, authenticates, reads AuJitter's identity and published package versions, and checks update readiness. This mode creates, uploads, deletes and commits no Store submission.

A validation against `v0.1.1` can check packaging and authentication, but update readiness must fail while Submission 1 is pending or once the same version is live. A newer released version is required for a full readiness pass.

The checks refuse an unknown app, corrupt or missing packages, the wrong publisher/architecture/version, a nonzero fourth MSIX version component, an unpublished first Store version, an existing pending submission, and a version that does not exceed all published packages. They do not replace or cancel an existing Store draft. If a submitted workflow fails or times out, inspect Partner Center before retrying: an upload or commit may already have happened.

## Release the next version

1. Update `Cargo.toml`, the lockfile and release notes to the intended version.
2. Ensure the first Store release is live, all four secrets are configured, and Partner Center has no pending draft or certification submission.
3. Push a matching stable tag, for example `v0.1.2` for version `0.1.2`.
4. The native workflow publishes the GitHub release after all package jobs pass, then automatically submits its Store bundle. Check the Store job's summary and Partner Center for certification results.

The direct reusable-workflow call also works when the GitHub release is created with `GITHUB_TOKEN`, which does not trigger another `release` workflow. Store runs are serialized and are not canceled by a new run. Tag builds are not canceled while publishing.

The local release checks can be exercised without credentials:

```sh
python -m unittest discover -s scripts/tests -p 'test_store*.py' -v
```

Tests cover incorrect/corrupt packages, both bundle architectures, version comparisons, pending submissions, non-mutating validation and credential redaction. Offline `prepare` additionally requires Windows SDK MakeAppx and the two released MSIX/checksum pairs under `<directory>/assets`.

References: [Microsoft's GitHub Actions setup](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/github-actions), [Store CLI commands](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/commands), [pinned CLI source](https://github.com/microsoft/msstore-cli/tree/v0.4.3).
