#!/usr/bin/env python3
"""Package a built native binary. Run on the target OS with Python 3.11+."""
import argparse
import hashlib
import platform
import plistlib
import shutil
import subprocess
import tarfile
import tomllib
import zipfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    system = platform.system()
    binary = args.binary or root / "target" / "release" / ("handypos.exe" if system == "Windows" else "handypos")
    if not binary.is_file():
        parser.error(f"Binary not found: {binary}. Run cargo build --release first.")
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    label = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}[system]
    name = f"HandyPOS-{version}-{label}-{platform.machine().lower()}"
    args.output.mkdir(parents=True, exist_ok=True)
    stage = args.output / name
    # A unique version/platform staging path; never remove an existing bundle.
    stage.mkdir(exist_ok=False)
    shutil.copy2(root / "README.md", stage / "README.md")
    if system == "Darwin":
        app = stage / "HandyPOS.app"
        contents = app / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        shutil.copy2(binary, contents / "MacOS" / "handypos")
        (contents / "MacOS" / "handypos").chmod(0o755)
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleName": "HandyPOS", "CFBundleDisplayName": "HandyPOS",
            "CFBundleIdentifier": "dev.handypos.desktop", "CFBundleExecutable": "handypos",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
            "CFBundleVersion": version, "LSMinimumSystemVersion": "11.0",
            "NSHighResolutionCapable": True,
        }))
        subprocess.run(["codesign", "--force", "--sign", "-", str(app)], check=True)
    else:
        target = stage / binary.name
        shutil.copy2(binary, target)
        if system == "Linux":
            target.chmod(0o755)
            (stage / "handypos.desktop").write_text(
                "[Desktop Entry]\nType=Application\nName=HandyPOS\n"
                "Comment=Local PostgreSQL workspace\nExec=handypos\n"
                "Terminal=false\nCategories=Development;Database;\n"
            )
    if system == "Linux":
        archive = args.output / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            output.add(stage, arcname=name)
    else:
        archive = args.output / f"{name}.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
            for path in sorted(stage.rglob("*")):
                output.write(path, path.relative_to(args.output))
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n")
    print(archive)


if __name__ == "__main__":
    main()
