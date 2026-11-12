#!/usr/bin/env python3
"""Refresh a complete DEB snapshot, or install the last good snapshot offline."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile


SCHEMA = 1


def command(args, **kwargs):
    return subprocess.run(args, check=True, text=True, env={**os.environ, "LC_ALL": "C"}, **kwargs)


def identity(packages):
    release = dict(
        line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines()
        if "=" in line and not line.startswith("#")
    )
    distro = release["ID"].strip('"')
    version = release["VERSION_ID"].strip('"')
    architecture = command(["dpkg", "--print-architecture"], capture_output=True).stdout.strip()
    if distro != "ubuntu" or not re.fullmatch(r"[a-z0-9.]+", version + architecture):
        raise ValueError("expected an Ubuntu runner and native Debian architecture")
    return {"schema": SCHEMA, "distro": distro, "version": version,
            "architecture": architecture, "requested": sorted(set(packages))}


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def prefix(context):
    return (f"linux-debs-v{SCHEMA}-{context['distro']}{context['version']}-"
            f"{context['architecture']}-{hashlib.sha256(encoded(context['requested'])).hexdigest()[:16]}")


def outputs(values):
    for key, value in values.items():
        print(f"{key}={value}", flush=True)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            for key, value in values.items():
                stream.write(f"{key}={value}\n")


def archive_names(plan):
    """APT --print-uris emits the archive basename it will actually write."""
    names = []
    for line in plan.splitlines():
        if not line.startswith("'"):
            continue
        parts = shlex.split(line)
        if len(parts) != 4 or not parts[1].endswith(".deb"):
            raise ValueError("unexpected APT archive plan")
        name = parts[1]
        if Path(name).name != name or not re.fullmatch(r"[A-Za-z0-9.+%_~:-]+\.deb", name):
            raise ValueError("unsafe APT archive filename")
        if name not in names:
            names.append(name)
    if not names:
        raise ValueError("APT did not produce a complete dependency download plan")
    return names


class Installer:
    def __init__(self, cache, context, run=command):
        self.cache = Path(cache).resolve()
        self.context = context
        self.run = run
        self.snapshot = self.cache / "snapshot"

    def describe(self, archive):
        control = self.run(["dpkg-deb", "--field", str(archive)], capture_output=True).stdout
        fields = dict(line.split(": ", 1) for line in control.splitlines()
                      if line and not line[0].isspace() and ": " in line)
        if fields["Architecture"] not in ("all", self.context["architecture"]):
            raise ValueError("wrong architecture in DEB snapshot")
        if any(key in fields for key in ("Filename", "Size", "SHA256")):
            raise ValueError("unexpected archive-index fields in DEB control")
        return {"filename": archive.name, "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                "size": archive.stat().st_size, "package": fields["Package"],
                "version": fields["Version"], "architecture": fields["Architecture"],
                "control": control.strip()}

    def load(self):
        try:
            manifest = json.loads((self.snapshot / "manifest.json").read_text())
            if manifest["context"] != self.context or not manifest["packages"]:
                raise ValueError("snapshot platform or requested packages differ")
            seen = set()
            for package in manifest["packages"]:
                name = package["filename"]
                if Path(name).name != name or not name.endswith(".deb") or name in seen:
                    raise ValueError("invalid snapshot archive name")
                seen.add(name)
                if self.describe(self.snapshot / "packages" / name) != package:
                    raise ValueError(f"snapshot integrity check failed: {name}")
            available = {p["package"] for p in manifest["packages"]}
            if not set(self.context["requested"]).issubset(available):
                raise ValueError("snapshot is missing requested packages")
            return manifest
        except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
            print(f"No usable DEB snapshot: {error}", flush=True)
            return None

    def apt(self, args, options=(), **kwargs):
        base = ["sudo", "apt-get", "-o", "Acquire::Retries=0",
                "-o", "Acquire::http::Timeout=5", "-o", "Acquire::https::Timeout=5"]
        return self.run([*base, *options, *args], **kwargs)

    def install_local(self, directory, manifest):
        """Only file: sources; current/partially refreshed remote indexes are excluded."""
        repo = directory / "repo"
        archives = repo / "archives"
        archives.mkdir(parents=True)
        (archives / "partial").mkdir()
        index = []
        for package in manifest["packages"]:
            shutil.copy2(directory / "downloads" / package["filename"], archives)
            index.append(f"{package['control']}\nFilename: archives/{package['filename']}\n"
                         f"Size: {package['size']}\nSHA256: {package['sha256']}\n")
        (repo / "Packages").write_text("\n".join(index) + "\n")
        sources = repo / "sources.list"
        # trusted=yes applies ONLY to this hash-checked local cache. Online
        # refresh still uses the runner's normal signed Ubuntu repositories.
        sources.write_text(f"deb [trusted=yes] {repo.as_uri()} ./\n")
        lists = repo / "lists"
        (lists / "partial").mkdir(parents=True)
        options = ["-o", f"Dir::Etc::sourcelist={sources}",
                   "-o", "Dir::Etc::sourceparts=-", "-o", f"Dir::State::lists={lists}",
                   "-o", f"Dir::Cache::archives={archives}", "-o", "Acquire::Languages=none"]
        self.apt(["update"], options)
        # APT resolves dependencies against the actual runner, but can only use
        # these DEBs and already-installed packages. Never force a downgrade.
        self.apt(["--yes", "--no-download", "--no-install-recommends", "install",
                  *self.context["requested"]], options)

    def publish(self, directory, manifest):
        staging = self.cache / "next-snapshot"
        shutil.rmtree(staging, ignore_errors=True)
        (staging / "packages").mkdir(parents=True)
        for package in manifest["packages"]:
            shutil.copy2(directory / "downloads" / package["filename"], staging / "packages")
        (staging / "manifest.json").write_bytes(encoded(manifest) + b"\n")
        backup = self.cache / "previous-snapshot"
        shutil.rmtree(backup, ignore_errors=True)
        if self.snapshot.exists():
            self.snapshot.rename(backup)
        staging.rename(self.snapshot)
        shutil.rmtree(backup, ignore_errors=True)

    def refresh(self, directory, previous):
        self.apt(["-o", "APT::Update::Error-Mode=any", "update"])
        downloads = directory / "downloads"
        downloads.mkdir()
        (downloads / "partial").mkdir()
        status = directory / "empty-status"
        status.touch()
        # Empty status downloads the entire dependency closure, including
        # packages already present on this runner but absent on the next one.
        options = ["-o", f"Dir::State::status={status}",
                   "-o", f"Dir::Cache::archives={downloads}"]
        install = ["--yes", "--download-only", "--no-install-recommends", "install",
                   *self.context["requested"]]
        plan = self.apt(["--print-uris", *install], options, capture_output=True).stdout
        names = archive_names(plan)
        old = {p["filename"]: p for p in previous["packages"]} if previous else {}
        reused = 0
        for name in names:
            if name in old:
                shutil.copy2(self.snapshot / "packages" / name, downloads / name)
                reused += 1
        print(f"DEB plan: {len(names)} archives; {reused} cached, {len(names) - reused} to download",
              flush=True)
        try:
            self.apt(install, options)
        finally:
            # APT may create root/_apt-owned partial files, including on failure.
            self.run(["sudo", "chown", "-R", f"{os.getuid()}:{os.getgid()}", str(downloads)])
        manifest = {"context": self.context,
                    "packages": [self.describe(downloads / name) for name in sorted(names)]}
        available = {p["package"] for p in manifest["packages"]}
        if not set(self.context["requested"]).issubset(available):
            raise ValueError("downloaded snapshot is missing requested packages")
        return manifest

    def execute(self):
        self.cache.mkdir(parents=True, exist_ok=True)
        previous = self.load()
        with tempfile.TemporaryDirectory(prefix="ft8rs-debs-", dir=self.cache) as work:
            directory = Path(work)
            directory.chmod(0o755)
            try:
                try:
                    manifest = self.refresh(directory, previous)
                    source = "updated"
                except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
                    if previous is None:
                        raise RuntimeError("APT refresh failed and no valid DEB cache is available") from error
                    print(f"APT refresh failed; installing last good DEB snapshot offline: {error}", flush=True)
                    shutil.rmtree(directory / "downloads", ignore_errors=True)
                    shutil.copytree(self.snapshot / "packages", directory / "downloads")
                    manifest = previous
                    source = "cached"
                # Installation failures are fatal, not a reason to retry after
                # potentially changing dpkg state. Keep the previous cache intact.
                self.install_local(directory, manifest)
                self.publish(directory, manifest)
            finally:
                self.run(["sudo", "chown", "-R", f"{os.getuid()}:{os.getgid()}", str(directory)])
        return {"cache-key": f"{prefix(self.context)}-{hashlib.sha256(encoded(manifest)).hexdigest()}",
                "source": source, "archives": len(manifest["packages"])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata-only", action="store_true")
    parser.add_argument("--cache-dir", type=Path, default=Path("dist/linux-deps"))
    parser.add_argument("packages", nargs="+")
    args = parser.parse_args()
    if any(not re.fullmatch(r"[a-z0-9][a-z0-9+.-]+", name) for name in args.packages):
        parser.error("expected plain Debian package names")
    context = identity(args.packages)
    if args.metadata_only:
        outputs({"cache-prefix": prefix(context)})
    else:
        outputs(Installer(args.cache_dir, context).execute())


if __name__ == "__main__":
    main()
