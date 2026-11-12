"""Network-fault/cache-state regressions; no sudo or APT installation on the host."""

import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from install_linux_deps import Installer, archive_names, prefix


CONTEXT = {"schema": 1, "distro": "ubuntu", "version": "24.04",
           "architecture": "amd64", "requested": ["example-dev"]}


def deb(name, version="1", architecture="amd64"):
    control = f"Package: {name}\nVersion: {version}\nArchitecture: {architecture}\n"
    if name == "example-dev":
        control += f"Depends: example-runtime (= {version})\n"
    return f"{name}_{version}_{architecture}.deb", control.encode()


class AptFixture:
    """Model the external APT boundary, including a dependency absent next run."""
    def __init__(self, version="1", update_fails=False, download_fails=False, install_fails=False):
        self.files = dict([deb("example-dev", version), deb("example-runtime", version),
                           deb("shared-data", architecture="all")])
        self.update_fails = update_fails
        self.download_fails = download_fails
        self.install_fails = install_fails
        self.calls = []
        self.downloaded = []
        self.installed = []

    def __call__(self, args, **kwargs):
        self.calls.append(args)
        result = subprocess.CompletedProcess(args, 0, stdout="")
        if args[:2] == ["dpkg-deb", "--field"]:
            result.stdout = Path(args[2]).read_text()
            return result
        if args[:2] == ["sudo", "chown"]:
            return result
        assert args[:2] == ["sudo", "apt-get"], args
        options = dict(args[i + 1].split("=", 1) for i, word in enumerate(args) if word == "-o")
        source = options.get("Dir::Etc::sourcelist")
        if "update" in args:
            if source:
                # Offline resolution must never see current remote lists/sources.
                assert options["Dir::Etc::sourceparts"] == "-"
                assert Path(source).read_text().startswith("deb [trusted=yes] file:")
                assert "Dir::State::lists" in options
            elif self.update_fails:
                raise subprocess.CalledProcessError(100, args)
            return result
        if "--download-only" in args:
            assert Path(options["Dir::State::status"]).read_text() == ""
            target = Path(options["Dir::Cache::archives"])
            if "--print-uris" in args:
                result.stdout = "\n".join(
                    f"'https://example.invalid/{name}' {name} {len(data)} MD5Sum:unused"
                    for name, data in self.files.items())
            else:
                if self.download_fails:
                    # Even a partially downloaded newer version must not poison fallback.
                    name, data = next(iter(self.files.items()))
                    (target / name).write_bytes(data)
                    raise subprocess.CalledProcessError(100, args)
                for name, data in self.files.items():
                    if not (target / name).exists():
                        (target / name).write_bytes(data)
                        self.downloaded.append(name)
            return result
        assert source and "install" in args and "--no-download" in args
        assert "--allow-downgrades" not in args
        if self.install_fails:
            raise subprocess.CalledProcessError(100, args)
        archives = Path(options["Dir::Cache::archives"])
        controls = [(p.name, p.read_text()) for p in archives.glob("*.deb")]
        # Independent expected dependency: caching only example-dev must fail.
        dev = next(text for _, text in controls if "Package: example-dev\n" in text)
        version = next(line.split(": ")[1] for line in dev.splitlines() if line.startswith("Version:"))
        assert (archives / f"example-runtime_{version}_amd64.deb").exists()
        self.installed.append(version)
        return result


class LinuxDependencyCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.cache = Path(self.temporary.name) / "cache"

    def install(self, system, context=CONTEXT):
        with contextlib.redirect_stdout(io.StringIO()):
            return Installer(self.cache, context, system).execute()

    def seed(self):
        system = AptFixture()
        result = self.install(system)
        return result

    def test_cold_online_downloads_complete_dependency_snapshot(self):
        system = AptFixture()
        result = self.install(system)
        self.assertEqual(result["source"], "updated")
        self.assertEqual(result["archives"], 3)
        self.assertEqual(system.installed, ["1"])
        self.assertEqual(len(system.downloaded), 3)
        manifest = json.loads((self.cache / "snapshot/manifest.json").read_text())
        self.assertEqual({p["package"] for p in manifest["packages"]},
                         {"example-dev", "example-runtime", "shared-data"})

    def test_healthy_unchanged_index_reuses_archives_and_same_cache_key(self):
        first = self.seed()
        system = AptFixture()
        second = self.install(system)
        self.assertEqual(first["cache-key"], second["cache-key"])
        self.assertEqual(system.downloaded, [])
        self.assertTrue(any("update" in call and "Dir::Etc::sourceparts=-" not in call
                            for call in system.calls))

    def test_healthy_network_installs_new_version_and_updates_snapshot(self):
        first = self.seed()
        system = AptFixture(version="2")
        second = self.install(system)
        self.assertNotEqual(first["cache-key"], second["cache-key"])
        self.assertEqual(system.installed, ["2"])
        self.assertEqual(len(system.downloaded), 2)  # shared-data is unchanged.
        self.assertFalse((self.cache / "snapshot/packages/example-dev_1_amd64.deb").exists())

    def test_failed_update_installs_old_snapshot_without_remote_download(self):
        first = self.seed()
        system = AptFixture(version="2", update_fails=True)
        second = self.install(system)
        self.assertEqual(second["source"], "cached")
        self.assertEqual(first["cache-key"], second["cache-key"])
        self.assertEqual(system.installed, ["1"])
        self.assertFalse(any("--download-only" in call for call in system.calls))

    def test_failed_new_download_ignores_new_index_and_partial_archive(self):
        first = self.seed()
        system = AptFixture(version="2", download_fails=True)
        second = self.install(system)
        self.assertEqual(second["source"], "cached")
        self.assertEqual(first["cache-key"], second["cache-key"])
        self.assertEqual(system.installed, ["1"])
        self.assertFalse((self.cache / "snapshot/packages/example-dev_2_amd64.deb").exists())

    def test_no_cache_and_failed_update_is_an_explicit_failure(self):
        system = AptFixture(update_fails=True)
        with self.assertRaisesRegex(RuntimeError, "no valid DEB cache"):
            self.install(system)
        self.assertEqual(system.installed, [])
        self.assertFalse((self.cache / "snapshot").exists())

    def test_corrupt_archive_is_not_used_as_offline_fallback(self):
        self.seed()
        (self.cache / "snapshot/packages/example-runtime_1_amd64.deb").write_text(
            deb("example-runtime", "9")[1].decode())
        system = AptFixture(update_fails=True)
        with self.assertRaisesRegex(RuntimeError, "no valid DEB cache"):
            self.install(system)
        self.assertEqual(system.installed, [])

    def test_missing_transitive_dependency_rejects_snapshot(self):
        self.seed()
        (self.cache / "snapshot/packages/example-runtime_1_amd64.deb").unlink()
        with self.assertRaisesRegex(RuntimeError, "no valid DEB cache"):
            self.install(AptFixture(update_fails=True))

    def test_different_platform_or_requests_cannot_use_cache(self):
        self.seed()
        for overrides in [{"version": "26.04"}, {"architecture": "arm64"},
                          {"requested": ["other-dev"]}]:
            context = {**CONTEXT, **overrides}
            self.assertNotEqual(prefix(context), prefix(CONTEXT))
            with self.assertRaisesRegex(RuntimeError, "no valid DEB cache"):
                self.install(AptFixture(update_fails=True), context)

    def test_install_conflict_does_not_fallback_or_replace_last_good_cache(self):
        self.seed()
        before = (self.cache / "snapshot/manifest.json").read_bytes()
        system = AptFixture(version="2", install_fails=True)
        with self.assertRaises(subprocess.CalledProcessError):
            self.install(system)
        self.assertEqual((self.cache / "snapshot/manifest.json").read_bytes(), before)
        self.assertEqual(sum("install" in call and "--no-download" in call
                             for call in system.calls), 1)

    def test_archive_plan_rejects_paths_and_missing_closure(self):
        for plan in ["", "'https://example.invalid/a.deb' ../a.deb 1 MD5Sum:x",
                     "'https://example.invalid/a.deb' a.deb 1"]:
            with self.assertRaises(ValueError):
                archive_names(plan)


if __name__ == "__main__":
    unittest.main()
