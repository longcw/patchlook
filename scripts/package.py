#!/usr/bin/env python3
"""Package a previously built binary; no signing, uploads or installation."""
import argparse
import json
import platform
import plistlib
import shutil
import tarfile
import tomllib
import zipfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--profile", choices=["debug", "release"], default="release")
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
package = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]
package_name = package["name"]
version = package["version"]
display_name = json.loads((root / "locales/en.json").read_text(encoding="utf-8"))["app_name"]
system = platform.system()
name = f"{package_name}.exe" if system == "Windows" else package_name
binary = root / "target" / args.profile / name
if not binary.is_file():
    parser.error(f"Build first: missing {binary}")
dist = root / "dist"
dist.mkdir(exist_ok=True)
artifact = f"{package_name}-{version}-{system.lower()}-{platform.machine().lower()}"

if system == "Darwin":
    app = dist / f"{display_name}.app"
    contents = app / "Contents"
    (contents / "MacOS").mkdir(parents=True, exist_ok=True)
    (contents / "Resources").mkdir(exist_ok=True)
    shutil.copy2(binary, contents / "MacOS" / package_name)
    info = {
        "CFBundleName": display_name,
        "CFBundleDisplayName": display_name,
        "CFBundleIdentifier": f"dev.{package_name}.app",
        "CFBundleExecutable": package_name,
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
    archive = dist / f"{artifact}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as out:
        for file in sorted(app.rglob("*")):
            if file.is_file():
                out.write(file, file.relative_to(dist))
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
