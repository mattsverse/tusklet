import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from homebrew import generate_cask, release_assets


class HomebrewReleaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.output = self.root / "Casks/handypos.rb"
        self.release = {"tag_name": "v1.2.3", "draft": False, "prerelease": False, "assets": []}
        self.filenames = [
            "HandyPOS_1.2.3_aarch64.dmg",
            "HandyPOS_1.2.3_x64.dmg",
            "handypos_1.2.3_x86_64.AppImage",
        ]
        for filename in self.filenames:
            content = filename.encode()
            (self.root / filename).write_bytes(content)
            checksum = hashlib.sha256(content).hexdigest()
            (self.root / (filename + ".sha256")).write_text(f"{checksum}  {filename}\n")
            for name in (filename, filename + ".sha256"):
                self.release["assets"].append({
                    "name": name,
                    "browser_download_url": f"https://github.com/mattsverse/handypos/releases/download/v1.2.3/{name}",
                })

    def generate(self):
        generate_cask(self.release, self.root, self.output)

    def test_generates_cask_for_both_operating_systems_with_verified_checksums(self):
        self.generate()
        cask = self.output.read_text()
        self.assertIn('version "1.2.3"', cask)
        self.assertIn('app "HandyPOS.app"', cask)
        self.assertIn('target: "HandyPOS.AppImage"', cask)
        self.assertIn('depends_on arch: :x86_64', cask)
        for filename in self.filenames:
            self.assertIn(hashlib.sha256(filename.encode()).hexdigest(), cask)
        self.generate()
        self.assertEqual(cask, self.output.read_text())

    def test_rejects_drafts_and_prereleases(self):
        for flag in ("draft", "prerelease"):
            with self.subTest(flag=flag):
                with self.assertRaisesRegex(ValueError, "published, stable"):
                    release_assets({**self.release, flag: True})

    def test_rejects_nonstable_tags(self):
        for tag in ("v1.2.3-beta.1", "1.2.3", "v01.2.3", "v1.2.3\n", 'v1.2.3"'):
            with self.subTest(tag=tag):
                with self.assertRaisesRegex(ValueError, "stable version"):
                    release_assets({**self.release, "tag_name": tag})

    def test_requires_all_architectures_and_checksums(self):
        for missing in self.release["assets"]:
            with self.subTest(missing=missing["name"]):
                incomplete = {**self.release, "assets": [a for a in self.release["assets"] if a != missing]}
                with self.assertRaisesRegex(ValueError, "exactly one"):
                    release_assets(incomplete)

    def test_rejects_assets_from_a_different_release(self):
        self.release["assets"][0]["browser_download_url"] = "https://example.com/download.dmg"
        with self.assertRaisesRegex(ValueError, "Unexpected download URL"):
            self.generate()
        self.assertFalse(self.output.exists())

    def test_corrupted_installer_does_not_replace_existing_cask(self):
        self.generate()
        original = self.output.read_text()
        (self.root / self.filenames[-1]).write_bytes(b"corrupted download")
        with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
            self.generate()
        self.assertEqual(original, self.output.read_text())

    def test_checksum_must_name_the_matching_installer(self):
        checksum = self.root / (self.filenames[0] + ".sha256")
        checksum.write_text(checksum.read_text().replace(self.filenames[0], self.filenames[1]))
        with self.assertRaisesRegex(ValueError, "Invalid checksum"):
            self.generate()

    def test_downgrade_does_not_replace_existing_cask(self):
        self.output.parent.mkdir()
        newer = 'cask "handypos" do\n  version "1.10.0"\nend\n'
        self.output.write_text(newer)
        with self.assertRaisesRegex(ValueError, "Refusing to downgrade"):
            self.generate()
        self.assertEqual(newer, self.output.read_text())

    def test_refuses_to_overwrite_unrecognized_cask(self):
        self.output.parent.mkdir()
        self.output.write_text('cask "handypos" do\n  version :latest\nend\n')
        with self.assertRaisesRegex(ValueError, "Cannot read the existing cask version"):
            self.generate()

    @unittest.skipUnless(shutil.which("brew"), "Homebrew is needed to validate the cask DSL")
    def test_homebrew_loads_each_platform_and_restricts_linux_architecture(self):
        self.generate()
        result = subprocess.run(
            ["brew", "ruby", "-e", '''
require "cask/cask_loader"
require "simulate_system"
content = File.read(ARGV.fetch(0))
variants = [[:macos, :arm], [:macos, :intel], [:linux, :intel], [:linux, :arm]].map do |os, arch|
  Homebrew::SimulateSystem.with(os: os, arch: arch) do
    cask = Cask::CaskLoader::FromContentLoader.new(content).load(config: nil)
    { url: cask.url.to_s, artifacts: cask.artifacts.map { |a| a.class.dsl_key }, arch: cask.depends_on.arch }
  end
end
puts variants.to_json
''', str(self.output)],
            env={**os.environ, "HOMEBREW_DEVELOPER": "1", "HOMEBREW_NO_AUTO_UPDATE": "1",
                 "HOMEBREW_NO_ANALYTICS": "1"},
            check=False, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        variants = json.loads(result.stdout)
        for variant, filename in zip(variants[:3], self.filenames):
            self.assertTrue(variant["url"].endswith("/" + filename))
        self.assertEqual(variants[0]["artifacts"], ["app"])
        self.assertEqual(variants[1]["artifacts"], ["app"])
        for variant in variants[2:]:
            self.assertEqual(variant["artifacts"], ["app_image", "binary"])
            self.assertEqual(variant["arch"], [{"type": "intel", "bits": 64}])


if __name__ == "__main__":
    unittest.main()
