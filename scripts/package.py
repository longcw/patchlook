#!/usr/bin/env python3
"""Package a built binary, optionally signing and notarizing macOS releases."""
import argparse
import json
import platform
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path


def create_macos_icon(source: Path, destination: Path) -> None:
    with tempfile.TemporaryDirectory() as directory:
        iconset = Path(directory) / "AppIcon.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = str(size * scale)
                suffix = "@2x" if scale == 2 else ""
                output = iconset / f"icon_{size}x{size}{suffix}.png"
                subprocess.run(
                    ["sips", "--resampleHeightWidth", pixels, pixels, str(source), "--out", str(output)],
                    check=True,
                    stdout=subprocess.DEVNULL,
                )
        subprocess.run(["iconutil", "--convert", "icns", str(iconset), "--output", str(destination)], check=True)


def create_macos_dmg(app: Path, destination: Path) -> None:
    with tempfile.TemporaryDirectory() as directory:
        staging = Path(directory)
        shutil.copytree(app, staging / app.name)
        (staging / "Applications").symlink_to("/Applications")
        subprocess.run(
            ["hdiutil", "create", "-volname", app.stem, "-srcfolder", str(staging),
             "-format", "UDZO", "-ov", str(destination)],
            check=True,
        )


def notarize_macos(archive: Path, profile: str, keychain: str | None) -> None:
    credentials = ["--keychain-profile", profile]
    if keychain:
        credentials.extend(["--keychain", keychain])
    result = subprocess.run(
        ["xcrun", "notarytool", "submit", str(archive), *credentials,
         "--wait", "--timeout", "30m", "--output-format", "json"],
        capture_output=True,
        text=True,
    )
    if result.returncode:
        print(result.stdout, end="")
        print(result.stderr, end="", file=sys.stderr)
        result.check_returncode()
    submission = json.loads(result.stdout)
    print(f"Notarization {submission['id']}: {submission['status']}", flush=True)
    if submission["status"] != "Accepted":
        subprocess.run(
            ["xcrun", "notarytool", "log", submission["id"], *credentials],
            check=False,
        )
        raise RuntimeError(f"Notarization failed: {submission['status']}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=["debug", "release"], default="release")
    parser.add_argument("--sign-identity", help="Developer ID Application signing identity (macOS only)")
    parser.add_argument("--notary-profile", help="Notary credentials already stored in a keychain (macOS only)")
    parser.add_argument("--notary-keychain", help="Keychain containing the notary profile")
    args = parser.parse_args()
    if args.notary_profile and (not args.sign_identity or args.sign_identity == "-"):
        parser.error("--notary-profile requires a Developer ID --sign-identity")
    if args.notary_keychain and not args.notary_profile:
        parser.error("--notary-keychain requires --notary-profile")
    root = Path(__file__).resolve().parents[1]
    package = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]
    package_name = package["name"]
    version = package["version"]
    display_name = json.loads((root / "locales/en.json").read_text(encoding="utf-8"))["app_name"]
    system = platform.system()
    if system != "Darwin" and (args.sign_identity or args.notary_profile):
        parser.error("Signing and notarization options require macOS")
    name = f"{package_name}.exe" if system == "Windows" else package_name
    binary = root / "target" / args.profile / name
    if not binary.is_file():
        parser.error(f"Build first: missing {binary}")
    dist = root / "dist"
    dist.mkdir(exist_ok=True)
    artifact = f"{package_name}-{version}-{system.lower()}-{platform.machine().lower()}"

    if system == "Darwin":
        app = dist / f"{display_name}.app"
        if app.exists():
            shutil.rmtree(app)
        contents = app / "Contents"
        (contents / "MacOS").mkdir(parents=True, exist_ok=True)
        (contents / "Resources").mkdir(exist_ok=True)
        shutil.copy2(binary, contents / "MacOS" / package_name)
        icon = contents / "Resources" / "AppIcon.icns"
        create_macos_icon(root / "assets" / "app-icon" / "patchlook-v2.png", icon)
        info = {
            "CFBundleName": display_name,
            "CFBundleDisplayName": display_name,
            "CFBundleIdentifier": f"dev.{package_name}.app",
            "CFBundleExecutable": package_name,
            "CFBundleIconFile": icon.name,
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "CFBundleDevelopmentRegion": "en",
            "CFBundleLocalizations": sorted(file.stem for file in (root / "locales").glob("*.json")),
            "NSHighResolutionCapable": True,
            "LSMinimumSystemVersion": "15.0",
        }
        with (contents / "Info.plist").open("wb") as stream:
            plistlib.dump(info, stream)
        signing = ["codesign", "--force", "--sign", args.sign_identity or "-"]
        if args.sign_identity:
            signing.extend(["--timestamp", "--options", "runtime"])
        subprocess.run([*signing, str(app)], check=True)
        subprocess.run(["codesign", "--verify", "--strict", str(app)], check=True)
        dmg = dist / f"{artifact}.dmg"
        create_macos_dmg(app, dmg)
        if args.sign_identity:
            subprocess.run(
                ["codesign", "--force", "--sign", args.sign_identity, "--timestamp", str(dmg)],
                check=True,
            )
        if args.notary_profile:
            notarize_macos(dmg, args.notary_profile, args.notary_keychain)
            for target in (dmg, app):
                subprocess.run(["xcrun", "stapler", "staple", str(target)], check=True)
                subprocess.run(["xcrun", "stapler", "validate", str(target)], check=True)
            subprocess.run(["spctl", "--assess", "--type", "execute", "--verbose=2", str(app)], check=True)
        archive = dist / f"{artifact}.zip"
        subprocess.run(["ditto", "-c", "-k", "--keepParent", str(app), str(archive)], check=True)
        print(dmg)
    elif system == "Windows":
        archive = dist / f"{artifact}.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as out:
            out.write(binary, name)
            out.write(root / "README.md", "README.md")
    else:
        archive = dist / f"{artifact}.tar.gz"
        with tarfile.open(archive, "w:gz") as out:
            out.add(binary, arcname=name)
            out.add(root / "README.md", arcname="README.md")
    print(archive)


if __name__ == "__main__":
    main()
