"""Validate released MSIX files and use Microsoft's pinned CLI for Store updates.

prepare is offline unless --download is supplied. check performs authentication
and Store reads only. publish is the only command that changes a submission.
Credentials and raw CLI responses are never printed or written to artifacts.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tomllib
import uuid
import xml.etree.ElementTree as ET
import zipfile


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "peppermintish/aujitter"
SECRET_NAMES = (
    "AZURE_AD_TENANT_ID",
    "AZURE_AD_APPLICATION_CLIENT_ID",
    "AZURE_AD_APPLICATION_SECRET",
    "SELLER_ID",
)
MACHINES = {"x64": 0x8664, "arm64": 0xAA64}


class ReleaseError(Exception):
    """A diagnostic that is safe to display in public CI logs."""


def version_tuple(value, parts=4):
    if not re.fullmatch(r"[0-9]+(?:\.[0-9]+){" + str(parts - 1) + r"}", value):
        raise ReleaseError("Invalid release/package version.")
    result = tuple(int(part) for part in value.split("."))
    if any(part > 65535 for part in result):
        raise ReleaseError("The version exceeds an MSIX version component limit.")
    return result


def release_version(tag):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ReleaseError("Use a stable release tag such as v0.1.2.")
    version_tuple(tag[1:], parts=3)
    return tag[1:]


def identity():
    return json.loads((ROOT / "packaging/windows/identity.json").read_text())


def run_capture(command, operation, timeout=120):
    # Do not use check=True: CalledProcessError and TimeoutExpired include argv,
    # and the configure command's argv contains the client secret.
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=timeout, env={**os.environ, "MSSTORE_OUTPUT_STREAM": "stderr"},
        )
    except (OSError, subprocess.TimeoutExpired):
        raise ReleaseError(f"{operation} could not complete. Check the tool and connection.") from None
    if result.returncode:
        raise ReleaseError(f"{operation} failed (exit {result.returncode}). Raw output is withheld to protect credentials.")
    return result.stdout


def cli_json(command, operation):
    try:
        data = json.loads(run_capture(command, operation))
    except json.JSONDecodeError:
        raise ReleaseError(f"{operation} did not return valid JSON.") from None
    if not isinstance(data, dict):
        raise ReleaseError(f"{operation} returned an unexpected response.")
    return data


def field(data, key, default=None):
    # CLI/model JSON casing differs between versions; names must still match.
    return next((v for k, v in data.items() if k.casefold() == key.casefold()), default)


def validate_credentials(environment):
    missing = [key for key in SECRET_NAMES if not environment.get(key, "").strip()]
    if missing:
        raise ReleaseError("Missing GitHub Actions secrets: " + ", ".join(missing))
    for key in ("AZURE_AD_TENANT_ID", "AZURE_AD_APPLICATION_CLIENT_ID"):
        try:
            parsed = uuid.UUID(environment[key])
        except (ValueError, AttributeError):
            raise ReleaseError(f"{key} must contain a GUID, not a display name.") from None
        if not parsed.int:
            raise ReleaseError(f"{key} must not contain an empty GUID.")
    if not re.fullmatch(r"[1-9][0-9]*", environment["SELLER_ID"]):
        raise ReleaseError("SELLER_ID must contain the numeric Partner Center seller ID, not the CN publisher ID.")


def verify_checksum(path):
    try:
        checksum = path.with_name(path.name + ".sha256").read_text().strip().split()
        with path.open("rb") as source:
            actual = hashlib.file_digest(source, "sha256").hexdigest()
    except OSError:
        raise ReleaseError(f"Missing package or checksum: {path.name}") from None
    if len(checksum) != 2 or checksum[1].lstrip("*") != path.name or checksum[0].lower() != actual:
        raise ReleaseError(f"Checksum mismatch: {path.name}")
    return actual


def xml_identity(archive, manifest="AppxManifest.xml"):
    try:
        root = ET.fromstring(archive.read(manifest))
        return next(node.attrib for node in root if node.tag.rsplit("}", 1)[-1] == "Identity")
    except (KeyError, ET.ParseError, StopIteration):
        raise ReleaseError("Package is missing a valid manifest identity.") from None


def verify_identity(actual, expected, version):
    if actual.get("Name") != expected["name"] or actual.get("Publisher") != expected["publisher"]:
        raise ReleaseError("Package identity does not match AuJitter's Store identity.")
    if actual.get("Version") != version + ".0":
        raise ReleaseError("Package version does not match the release tag (fourth component must be zero).")


def verify_pe(source, architecture):
    header = source.read(64)
    if len(header) != 64 or header[:2] != b"MZ":
        raise ReleaseError("Package contains an invalid Windows executable.")
    offset = struct.unpack_from("<I", header, 60)[0]
    if offset < 64 or offset > 16 * 1024 * 1024:
        raise ReleaseError("Package contains an invalid PE header offset.")
    source.seek(offset)
    pe = source.read(6)
    if len(pe) != 6 or pe[:4] != b"PE\0\0" or struct.unpack_from("<H", pe, 4)[0] != MACHINES[architecture]:
        raise ReleaseError(f"Executable architecture does not match {architecture}.")


def verify_package(path, expected, version, architecture):
    try:
        with zipfile.ZipFile(path) as archive:
            if archive.testzip():
                raise ReleaseError("MSIX archive integrity check failed.")
            actual = xml_identity(archive)
            verify_identity(actual, expected, version)
            if actual.get("ProcessorArchitecture") != architecture:
                raise ReleaseError("MSIX architecture does not match its release filename.")
            for executable in ("aujitter.exe", "aujitter-gui.exe"):
                with archive.open(executable) as source:
                    verify_pe(source, architecture)
    except (zipfile.BadZipFile, KeyError, OSError):
        raise ReleaseError(f"Invalid or incomplete MSIX: {path.name}") from None


def verify_bundle(path, expected, version):
    try:
        with zipfile.ZipFile(path) as archive:
            if archive.testzip():
                raise ReleaseError("Bundle archive integrity check failed.")
            verify_identity(xml_identity(archive, "AppxMetadata/AppxBundleManifest.xml"), expected, version)
            root = ET.fromstring(archive.read("AppxMetadata/AppxBundleManifest.xml"))
            packages = [node for node in root.iter() if node.tag.rsplit("}", 1)[-1] == "Package"]
            if len(packages) != 2 or {p.get("Architecture") for p in packages} != set(MACHINES):
                raise ReleaseError("The Store bundle must include exactly x64 and ARM64 application packages.")
            for package in packages:
                if package.get("Type") != "application" or package.get("Version") != version + ".0":
                    raise ReleaseError("Bundle package type/version mismatch.")
                filename = package.get("FileName", "")
                with archive.open(filename) as source, zipfile.ZipFile(source) as inner:
                    verify_identity(xml_identity(inner), expected, version)
                    if xml_identity(inner).get("ProcessorArchitecture") != package.get("Architecture"):
                        raise ReleaseError("Bundle architecture does not match its inner package.")
                    if inner.testzip():
                        raise ReleaseError("Bundled MSIX archive integrity check failed.")
                    for executable in ("aujitter.exe", "aujitter-gui.exe"):
                        with inner.open(executable) as binary:
                            verify_pe(binary, package.get("Architecture"))
    except (zipfile.BadZipFile, KeyError, OSError, ET.ParseError):
        raise ReleaseError("Invalid or incomplete Store bundle.") from None


def download_release(tag, assets):
    version = release_version(tag)
    release = cli_json(
        ["gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "tagName,isDraft,isPrerelease,assets"],
        "GitHub release lookup",
    )
    if release.get("tagName") != tag or release.get("isDraft") or release.get("isPrerelease"):
        raise ReleaseError("Store uploads require a published stable GitHub release.")
    cargo = run_capture(["git", "show", f"{tag}:Cargo.toml"], "Release source lookup")
    if tomllib.loads(cargo)["package"]["version"] != version:
        raise ReleaseError("Release tag and the released Cargo.toml version differ.")
    names = {asset["name"] for asset in release["assets"]}
    wanted = {
        f"AuJitter-{version}-windows-{arch}-store.msix{suffix}"
        for arch in MACHINES for suffix in ("", ".sha256")
    }
    if not wanted.issubset(names):
        raise ReleaseError("The GitHub release is missing a Windows MSIX or its checksum.")
    for name in sorted(wanted):
        run_capture(
            ["gh", "release", "download", tag, "--repo", REPOSITORY, "--pattern", name, "--dir", str(assets), "--clobber"],
            "GitHub package download", timeout=300,
        )


def prepare(tag, directory, download=False):
    version = release_version(tag)
    assets = directory / "assets"
    if download:
        assets.mkdir(parents=True, exist_ok=True)
        download_release(tag, assets)
    expected = identity()
    packages = []
    for architecture in MACHINES:
        path = assets / f"AuJitter-{version}-windows-{architecture}-store.msix"
        verify_checksum(path)
        verify_package(path, expected, version, architecture)
        packages.append(path)
    sdk = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Windows Kits/10/bin"
    candidates = sorted(sdk.glob("*/x64/makeappx.exe"))
    makeappx = shutil.which("makeappx") or (str(candidates[-1]) if candidates else None)
    if not makeappx:
        raise ReleaseError("Bundling requires Windows SDK MakeAppx on a Windows runner.")
    stage = directory / "staging" / version
    output = directory / "bundle"
    stage.mkdir(parents=True, exist_ok=True)
    output.mkdir(parents=True, exist_ok=True)
    for package in packages:
        shutil.copy2(package, stage / package.name)
    if set(stage.iterdir()) != {stage / package.name for package in packages}:
        raise ReleaseError("The bundle staging folder contains unexpected files.")
    bundle = output / f"AuJitter-{version}-store.msixbundle"
    run_capture(
        [makeappx, "bundle", "/o", "/d", str(stage), "/p", str(bundle), "/bv", version + ".0"],
        "Windows SDK bundle validation", timeout=300,
    )
    verify_bundle(bundle, expected, version)
    with bundle.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    bundle.with_name(bundle.name + ".sha256").write_text(f"{digest}  {bundle.name}\n")
    (output / "validation.json").write_text(json.dumps({
        "tag": tag, "version": version + ".0", "store_id": expected["store_id"],
        "architectures": list(MACHINES), "bundle": bundle.name, "sha256": digest,
    }, indent=2) + "\n")
    print(f"Validated release checksums, Store identity and native x64/ARM64 executables; created {bundle.name}.")
    return bundle


def configure_store():
    validate_credentials(os.environ)
    run_capture(["msstore", "settings", "--enableTelemetry", "false"], "Store CLI settings")
    run_capture([
        "msstore", "reconfigure", "--tenantId", os.environ[SECRET_NAMES[0]],
        "--clientId", os.environ[SECRET_NAMES[1]], "--clientSecret", os.environ[SECRET_NAMES[2]],
        "--sellerId", os.environ[SECRET_NAMES[3]],
    ], "Store credential configuration")


def verify_store_application(application, expected):
    checks = {
        "id": expected["store_id"], "packageIdentityName": expected["name"],
        "packageFamilyName": expected["package_family_name"], "publisherName": expected["publisher"],
    }
    for key, wanted in checks.items():
        if field(application, key) != wanted:
            raise ReleaseError(f"Store application {key} does not match AuJitter's checked-in identity.")


def verify_update_ready(application, submission, version):
    if not field(application, "lastPublishedApplicationSubmission"):
        raise ReleaseError("The first Store version is not live yet. Wait for its certification and publication before automated updates.")
    if field(application, "pendingApplicationSubmission"):
        raise ReleaseError("Partner Center already has a pending submission. Resolve it there; CI will not replace or cancel it.")
    packages = field(submission, "applicationPackages", [])
    if not packages:
        raise ReleaseError("The live Store submission has no package versions to compare.")
    if version_tuple(version + ".0") <= max(version_tuple(field(p, "version", "")) for p in packages):
        raise ReleaseError("The next Store package version must be newer than every published package.")


def check_store(tag, directory):
    version = release_version(tag)
    expected = identity()
    bundle = directory / "bundle" / f"AuJitter-{version}-store.msixbundle"
    verify_checksum(bundle)
    verify_bundle(bundle, expected, version)
    configure_store()
    application = cli_json(["msstore", "apps", "get", expected["store_id"]], "Store application read/authentication")
    verify_store_application(application, expected)
    print("Store credentials authenticated and the API returned AuJitter's matching identity.")
    # Refuse before submission get/publish so an existing draft cannot be touched.
    if not field(application, "lastPublishedApplicationSubmission"):
        verify_update_ready(application, {}, version)
    if field(application, "pendingApplicationSubmission"):
        verify_update_ready(application, {}, version)
    submission = cli_json(["msstore", "submission", "get", expected["store_id"]], "Published Store submission read")
    verify_update_ready(application, submission, version)
    print("Store authentication and AuJitter identity verified; no pending submission; package version is newer. No Store changes made.")
    return bundle


def publish(tag, directory):
    # Recheck immediately before the only mutating call, including on retries.
    bundle = check_store(tag, directory)
    expected = identity()
    run_capture([
        "msstore", "publish", str(bundle), "--appId", expected["store_id"],
        "--priceId", "Free", "--uploadTimeout", "600",
    ], "Store upload/submission (inspect Partner Center before retrying if this failed)", timeout=900)
    application = cli_json(["msstore", "apps", "get", expected["store_id"]], "Submitted Store application read")
    submission = field(application, "pendingApplicationSubmission") or field(application, "lastPublishedApplicationSubmission")
    if not submission or not field(submission, "id"):
        raise ReleaseError("The publish command completed but a submission ID could not be verified. Inspect Partner Center before retrying.")
    print(f"Store submission {field(submission, 'id')} sent with x64 and ARM64. Microsoft certification/publication is still required.")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as output:
            output.write(f"Submitted **AuJitter {tag}**, x64 and ARM64, to Microsoft Store certification.\n\n[Check Partner Center](https://partner.microsoft.com/en-us/dashboard/products/{expected['store_id']}/overview). Certification has not been claimed as passed.\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "credentials", "check", "publish"))
    parser.add_argument("--tag")
    parser.add_argument("--directory", type=Path, default=Path("store-upload"))
    parser.add_argument("--download", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "credentials":
            validate_credentials(os.environ)
            print("All four required secret names are present and their ID formats are valid. Authentication is checked separately.")
        else:
            if not args.tag:
                raise ReleaseError("A release tag is required.")
            if args.command == "prepare":
                prepare(args.tag, args.directory, args.download)
            elif args.command == "check":
                check_store(args.tag, args.directory)
            else:
                publish(args.tag, args.directory)
    except ReleaseError as error:
        print(f"Store release check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
