"""Version identity and actual Linux/Windows archive contract regressions."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

from package_release import SEMVER, build_metadata, package


class PackageReleaseTests(unittest.TestCase):
    def env(self, **overrides):
        return {
            "GITHUB_SHA": "97e82bd12345" + "a" * 28,
            "GITHUB_RUN_ID": "40699594370",
            "GITHUB_RUN_NUMBER": "31",
            "GITHUB_RUN_ATTEMPT": "1",
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REF_TYPE": "branch",
            "GITHUB_REF": "refs/heads/codex/udp-fix",
            "GITHUB_REF_NAME": "codex/udp-fix",
            "GITHUB_REPOSITORY": "tallcode/ft8rs",
            **overrides,
        }

    def test_manual_branch_and_main_builds_have_traceable_dev_versions(self):
        for ref in ["main", "codex/udp-fix"]:
            info = build_metadata(self.env(GITHUB_REF=f"refs/heads/{ref}", GITHUB_REF_NAME=ref), "windows")
            self.assertEqual(info["version"], "0.0.0-dev.31.g97e82bd12345")
            self.assertEqual(info["run_number"], "31")
            self.assertEqual(info["run_id"], "40699594370")
            self.assertIsNotNone(SEMVER.fullmatch(info["version"]))
            self.assertEqual(info["ref"], f"refs/heads/{ref}")
            self.assertEqual(info["commit"], self.env()["GITHUB_SHA"])
            self.assertEqual(info["run_url"], "https://github.com/tallcode/ft8rs/actions/runs/40699594370")
            rerun = build_metadata(self.env(GITHUB_RUN_ATTEMPT="2", GITHUB_REF=f"refs/heads/{ref}", GITHUB_REF_NAME=ref), "windows")
            self.assertEqual(info, rerun)

    def test_only_tag_pushes_get_release_identity_and_stable_filenames(self):
        tag = self.env(GITHUB_REF="refs/tags/v0.0.23", GITHUB_REF_NAME="v0.0.23", GITHUB_REF_TYPE="tag")
        manual = build_metadata(tag, "linux")
        self.assertEqual(manual["channel"], "dev")
        self.assertIn(manual["version"], manual["archive_name"])
        release = build_metadata({**tag, "GITHUB_EVENT_NAME": "push"}, "linux")
        self.assertEqual(release["version"], "0.0.23")
        self.assertEqual(release["archive_name"], "ft8rs-linux-x86_64.tar.gz")
        main = build_metadata(self.env(GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/main", GITHUB_REF_NAME="main"), "linux")
        self.assertEqual(main["channel"], "dev")

    def test_rejects_invalid_source_and_semver_tags(self):
        for fields in [
            {"GITHUB_SHA": "main"},
            {"GITHUB_RUN_ID": "01"},
            {"GITHUB_RUN_ID": ""},
            {"GITHUB_RUN_NUMBER": "01"},
            {"GITHUB_RUN_NUMBER": ""},
        ]:
            with self.assertRaises(ValueError):
                build_metadata(self.env(**fields), "linux")
        for tag in ["v1.2.x", "v01.2.3", "v1.2.3-dev.01", "v1.2.3-dev..7"]:
            with self.assertRaises(ValueError):
                build_metadata(self.env(GITHUB_EVENT_NAME="push", GITHUB_REF_TYPE="tag", GITHUB_REF_NAME=tag), "linux")

    def test_actions_output_distinguishes_stable_and_prerelease_tags(self):
        for tag, expected in [
            ("v1.2.3", "false"),
            ("v1.2.3-rc.1", "true"),
            ("v1.2.3+build-4", "false"),
        ]:
            with self.subTest(tag=tag), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env = {
                    **os.environ,
                    **self.env(
                        GITHUB_EVENT_NAME="push", GITHUB_REF_TYPE="tag",
                        GITHUB_REF=f"refs/tags/{tag}", GITHUB_REF_NAME=tag,
                    ),
                    "GITHUB_OUTPUT": str(root / "outputs"),
                    "GITHUB_STEP_SUMMARY": str(root / "summary"),
                }
                subprocess.run(
                    [sys.executable, "-B", str(Path(__file__).with_name("package_release.py")),
                     "metadata", "--platform", "linux", "--output", str(root / "metadata.json")],
                    env=env, check=True, capture_output=True,
                )
                outputs = dict(line.split("=", 1) for line in (root / "outputs").read_text().splitlines())
                self.assertEqual(outputs["release_tag"], tag)
                self.assertEqual(outputs["prerelease"], expected)

    def test_archives_contain_only_the_platform_binaries_and_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "target/release"
            binaries.mkdir(parents=True)
            for binary in ["ft8rs", "ft8rs-gui", "ft8rs.exe", "ft8rs-gui.exe"]:
                (binaries / binary).write_bytes(binary.encode())
            database = root / "crates/ft8rs-core/ALLCALL7.TXT"
            database.parent.mkdir(parents=True)
            database.write_bytes(b"private test database")
            for platform in ["linux", "windows"]:
                info = build_metadata(self.env(), platform)
                archive = package(root, info, root / "dist")
                prefix = info["package_name"] + "/"
                if platform == "linux":
                    with tarfile.open(archive) as contents:
                        files = {m.name: contents.extractfile(m).read() for m in contents if m.isfile()}
                        self.assertTrue(contents.getmember(prefix + "ft8rs").mode & 0o111)
                    expected = {"ft8rs"}
                else:
                    with zipfile.ZipFile(archive) as contents:
                        files = {name: contents.read(name) for name in contents.namelist() if not name.endswith("/")}
                    expected = {"ft8rs.exe", "ft8rs-gui.exe"}
                self.assertEqual(set(files), {prefix + name for name in expected | {"ALLCALL7.TXT", "BUILD-INFO.json", "README.txt"}})
                self.assertEqual(json.loads(files[prefix + "BUILD-INFO.json"]), info)
                digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                self.assertEqual(Path(str(archive) + ".sha256").read_text(), f"{digest}  {archive.name}\n")


if __name__ == "__main__":
    unittest.main()
