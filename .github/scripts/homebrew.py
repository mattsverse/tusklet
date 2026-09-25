"""Generate the tap cask from a published release and its verified installers."""

import argparse
import hashlib
import json
from pathlib import Path
import re
from string import Template


REPOSITORY = "mattsverse/tusklet"
VERSION_PATTERN = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
TEMPLATE = Path(__file__).resolve().parents[1] / "homebrew/tusklet.rb.template"


def release_assets(release):
    """Reject unpublished, unstable, or incomplete releases before downloading."""
    if release.get("draft") is not False or release.get("prerelease") is not False:
        raise ValueError("Homebrew publishing requires a published, stable release")
    tag = release.get("tag_name", "")
    if not re.fullmatch("v" + VERSION_PATTERN, tag):
        raise ValueError("Release tag must be a stable version such as v1.2.3")
    version = tag[1:]
    packages = {
        "MAC_ARM_SHA256": f"Tusklet_{version}_aarch64.dmg",
        "MAC_INTEL_SHA256": f"Tusklet_{version}_x64.dmg",
        "LINUX_SHA256": f"tusklet_{version}_x86_64.AppImage",
    }
    assets = release.get("assets", [])
    for filename in packages.values():
        for name in (filename, filename + ".sha256"):
            matches = [asset for asset in assets if asset.get("name") == name]
            if len(matches) != 1:
                raise ValueError(f"Release must contain exactly one {name}")
            expected_url = f"https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"
            if matches[0].get("browser_download_url") != expected_url:
                raise ValueError(f"Unexpected download URL for {name}")
    return version, packages


def generate_cask(release, assets_dir, output):
    version, packages = release_assets(release)
    if output.exists():
        previous = re.search(r'^  version "(' + VERSION_PATTERN + r')"$', output.read_text(), re.M)
        if previous is None:
            raise ValueError("Cannot read the existing cask version; refusing to overwrite it")
        if tuple(map(int, version.split("."))) < tuple(map(int, previous[1].split("."))):
            raise ValueError(f"Refusing to downgrade the cask from {previous[1]} to {version}")

    substitutions = {"VERSION": version}
    for key, filename in packages.items():
        package = assets_dir / filename
        checksum = (assets_dir / (filename + ".sha256")).read_text(encoding="utf-8-sig").strip()
        match = re.fullmatch(r"([0-9a-fA-F]{64})  " + re.escape(filename), checksum)
        if match is None:
            raise ValueError(f"Invalid checksum file for {filename}")
        with package.open("rb") as source:
            actual = hashlib.file_digest(source, "sha256").hexdigest()
        if actual != match[1].lower():
            raise ValueError(f"Checksum mismatch for {filename}")
        substitutions[key] = actual

    content = Template(TEMPLATE.read_text()).substitute(substitutions)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(content)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path, help="GitHub release API JSON")
    parser.add_argument("--assets", type=Path, help="Directory of downloaded release assets")
    parser.add_argument("--output", type=Path, help="Destination Casks/tusklet.rb")
    args = parser.parse_args()
    if (args.assets is None) != (args.output is None):
        parser.error("--assets and --output must be supplied together")
    try:
        release = json.loads(args.release.read_text())
        if args.output is None:
            version, _ = release_assets(release)
            print(f"Validated Tusklet {version} release assets")
        else:
            generate_cask(release, args.assets, args.output)
            print(f"Generated {args.output}")
    except (ValueError, OSError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
