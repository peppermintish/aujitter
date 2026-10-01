"""Create native packages from the two already-built Rust binaries (stdlib only)."""
import argparse
import hashlib
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tomllib
import json
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def fresh_stage(name):
    path = (ROOT / "build" / name).resolve()
    if not path.is_relative_to((ROOT / "build").resolve()):
        raise ValueError("Stage must stay inside the workspace build directory")
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    return path


def copy_docs(stage):
    shutil.copy2(ROOT / "LICENSE", stage)
    shutil.copy2(ROOT / "README.md", stage)
    shutil.copytree(ROOT / "docs", stage / "docs")


def windows(stage, binaries, name, version, arch):
    for binary in ("aujitter.exe", "aujitter-gui.exe"):
        shutil.copy2(binaries / binary, stage)
    copy_docs(stage)
    shutil.make_archive(str(ROOT / "dist" / name), "zip", stage)
    assets = stage / "Assets"
    assets.mkdir()
    for asset in ("StoreLogo.png", "Square44x44Logo.png", "Square150x150Logo.png"):
        shutil.copy2(ROOT / "packaging" / "assets" / asset, assets)
    identity = json.loads((ROOT / "packaging/windows/identity.json").read_text())
    template = (ROOT / "packaging/windows/AppxManifest.xml.in").read_text()
    tokens = {"NAME": identity["name"], "PUBLISHER": identity["publisher"],
              "PUBLISHER_DISPLAY": identity["publisher_display_name"],
              "ARCH": arch, "VERSION": version + ".0"}
    for key, value in tokens.items():
        template = template.replace("@" + key + "@", escape(value, {'"': '&quot;'}))
    (stage / "AppxManifest.xml").write_text(template, encoding="utf-8")
    sdk = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Windows Kits/10/bin"
    candidates = sorted(sdk.glob("*/x64/makeappx.exe"))
    tool = shutil.which("makeappx") or (candidates[-1] if candidates else None)
    if not tool:
        raise RuntimeError("MakeAppx requires the Windows SDK")
    run(tool, "pack", "/o", "/d", stage, "/p", ROOT / "dist" / (name + "-store.msix"))


def macos(stage, binaries, name, version, arch):
    app = stage / "AuJitter.app"
    contents = app / "Contents"
    exe = contents / "MacOS"
    resources = contents / "Resources"
    exe.mkdir(parents=True)
    resources.mkdir()
    for binary in ("aujitter", "aujitter-gui"):
        shutil.copy2(binaries / binary, exe)
        (exe / binary).chmod(0o755)
    iconset = stage / "AuJitter.iconset"
    iconset.mkdir()
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            target = iconset / f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
            run("sips", "-z", size * scale, size * scale, ROOT / "packaging/assets/icon1024.png", "--out", target)
    run("iconutil", "-c", "icns", iconset, "-o", resources / "AuJitter.icns")
    shutil.rmtree(iconset)
    info = {"CFBundleName": "AuJitter", "CFBundleDisplayName": "AuJitter",
            "CFBundleIdentifier": "au.aujitter.app", "CFBundleVersion": version,
            "CFBundleShortVersionString": version, "CFBundleExecutable": "aujitter-gui",
            "CFBundleIconFile": "AuJitter.icns", "CFBundlePackageType": "APPL",
            "LSMinimumSystemVersion": "12.0", "NSHighResolutionCapable": True,
            "NSLocalNetworkUsageDescription": "AuJitter checks your local router and optionally reads its status."}
    with (contents / "Info.plist").open("wb") as output:
        plistlib.dump(info, output)
    copy_docs(resources)
    # Ad-hoc signing preserves the ARM64 executable signature. Distribution signing is optional later.
    run("codesign", "--force", "--deep", "--sign", "-", app)
    os.symlink("/Applications", stage / "Applications")
    run("hdiutil", "create", "-volname", "AuJitter", "-srcfolder", stage,
        "-ov", "-format", "UDZO", ROOT / "dist" / (name + ".dmg"))
    run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", app, ROOT / "dist" / (name + ".zip"))


def linux(stage, binaries, name, version, arch):
    portable = stage / name
    portable.mkdir()
    for binary in ("aujitter", "aujitter-gui"):
        shutil.copy2(binaries / binary, portable)
        (portable / binary).chmod(0o755)
    copy_docs(portable)
    with tarfile.open(ROOT / "dist" / (name + ".tar.gz"), "w:gz") as output:
        output.add(portable, arcname=name)
    package = stage / "deb"
    control = package / "DEBIAN"
    control.mkdir(parents=True)
    installed = package / "usr/bin"
    installed.mkdir(parents=True)
    for binary in ("aujitter", "aujitter-gui"):
        shutil.copy2(portable / binary, installed)
    desktop = package / "usr/share/applications"
    desktop.mkdir(parents=True)
    shutil.copy2(ROOT / "packaging/linux/aujitter.desktop", desktop)
    icons = package / "usr/share/icons/hicolor/scalable/apps"
    icons.mkdir(parents=True)
    shutil.copy2(ROOT / "packaging/icon.svg", icons / "aujitter.svg")
    doc = package / "usr/share/doc/aujitter"
    doc.mkdir(parents=True)
    shutil.copy2(ROOT / "LICENSE", doc / "copyright")
    deps = "libc6 (>= 2.39), libfontconfig1, libfreetype6, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libxcb-render0, libxcb-shape0, libxcb-xfixes0, libwayland-client0, libvulkan1, iputils-ping, ca-certificates"
    text = f"Package: aujitter\nVersion: {version}\nArchitecture: {'amd64' if arch == 'x64' else 'arm64'}\nMaintainer: peppermintish\nSection: net\nPriority: optional\nDepends: {deps}\nDescription: Local network monitoring with CLI, web and native GPUI interfaces\n Logs measured interruptions and evidence with a low traffic budget.\n"
    (control / "control").write_text(text)
    run("dpkg-deb", "--root-owner-group", "--build", package, ROOT / "dist" / (name + ".deb"))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "macos", "linux"], required=True)
    parser.add_argument("--arch", choices=["x64", "arm64"], required=True)
    parser.add_argument("--profile", default="release", choices=["release", "debug"])
    parser.add_argument("--target")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    name = f"AuJitter-{version}-{args.platform}-{args.arch}"
    binaries = ROOT / "target"
    if args.target:
        binaries /= args.target
    binaries /= args.profile
    (ROOT / "dist").mkdir(exist_ok=True)
    stage = fresh_stage(name)
    globals()[args.platform](stage, binaries, name, version, args.arch)
    for file in sorted((ROOT / "dist").glob(name + ".*")) + sorted((ROOT / "dist").glob(name + "-store.msix")):
        digest = hashlib.file_digest(file.open("rb"), "sha256").hexdigest()
        file.with_name(file.name + ".sha256").write_text(f"{digest}  {file.name}\n")
        print(file.name)


if __name__ == "__main__":
    main()
