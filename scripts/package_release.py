#!/usr/bin/env python3
"""Prepare traceable CI versions and package only the binaries each OS ships."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import tempfile


NUMBER = r"(?:0|[1-9][0-9]*)"
PRERELEASE = r"(?:0|[1-9][0-9]*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER = re.compile(
    rf"{NUMBER}\.{NUMBER}\.{NUMBER}"
    rf"(?:-{PRERELEASE}(?:\.{PRERELEASE})*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def build_metadata(env, platform):
    if platform not in ("linux", "windows"):
        raise ValueError("only Linux CLI and Windows CLI + GUI are packaged")
    sha = env["GITHUB_SHA"]
    run_id = env["GITHUB_RUN_ID"]
    run_number = env["GITHUB_RUN_NUMBER"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("expected the full source commit SHA")
    if not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("expected a positive Actions run ID")
    if not re.fullmatch(r"[1-9][0-9]*", run_number):
        raise ValueError("expected a positive workflow build number")
    # A manual dispatch always produces development artifacts, even if its ref
    # happens to be a tag. Only a tag push is an official release.
    release = env["GITHUB_EVENT_NAME"] == "push" and env["GITHUB_REF_TYPE"] == "tag"
    if release:
        tag = env["GITHUB_REF_NAME"]
        if not tag.startswith("v") or not SEMVER.fullmatch(tag[1:]):
            raise ValueError("release tags must be v-prefixed SemVer versions")
        version = tag[1:]
    else:
        version = f"0.0.0-dev.{run_number}.g{sha[:12]}"
    package_name = f"ft8rs-{platform}-x86_64"
    archive_stem = package_name if release else f"{package_name}-{version}"
    return {
        "version": version,
        "channel": "release" if release else "dev",
        "commit": sha,
        "ref": env["GITHUB_REF"],
        "repository": env["GITHUB_REPOSITORY"],
        "run_id": run_id,
        "run_number": run_number,
        "run_url": (
            f"{env.get('GITHUB_SERVER_URL', 'https://github.com')}/"
            f"{env['GITHUB_REPOSITORY']}/actions/runs/{run_id}"
        ),
        "platform": platform,
        "package_name": package_name,
        "archive_name": archive_stem + (".zip" if platform == "windows" else ".tar.gz"),
    }


def package(root, info, output):
    output.mkdir(parents=True, exist_ok=True)
    windows = info["platform"] == "windows"
    binaries = ["ft8rs.exe", "ft8rs-gui.exe"] if windows else ["ft8rs"]
    with tempfile.TemporaryDirectory(prefix="ft8rs-package-", dir=output) as work:
        staging = Path(work) / info["package_name"]
        staging.mkdir()
        for binary in binaries:
            destination = staging / binary
            shutil.copy2(root / "target/release" / binary, destination)
            if not windows:
                destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        shutil.copy2(root / "crates/ft8rs-core/ALLCALL7.TXT", staging / "ALLCALL7.TXT")
        (staging / "BUILD-INFO.json").write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
        (staging / "README.txt").write_text(
            f"ft8.rs {info['version']} ({info['channel']})\n\n"
            + "\n".join(binaries)
            + "\n\nKeep ALLCALL7.TXT next to the binaries.\n"
            "This build uses RustFFT and has no FFTW runtime dependency.\n"
            f"Source: {info['commit']}\nRef: {info['ref']}\nBuild: {info['run_url']}\n",
            encoding="utf-8",
        )
        suffix = ".zip" if windows else ".tar.gz"
        archive = output / info["archive_name"]
        shutil.make_archive(
            str(archive)[:-len(suffix)], "zip" if windows else "gztar",
            root_dir=work, base_dir=info["package_name"],
        )
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    Path(str(archive) + ".sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    metadata = commands.add_parser("metadata")
    metadata.add_argument("--platform", choices=("linux", "windows"), required=True)
    metadata.add_argument("--output", type=Path, required=True)
    archive = commands.add_parser("package")
    archive.add_argument("--metadata", type=Path, required=True)
    archive.add_argument("--root", type=Path, default=Path.cwd())
    archive.add_argument("--output-dir", type=Path, default=Path("dist"))
    args = parser.parse_args()
    if args.command == "metadata":
        info = build_metadata(os.environ, args.platform)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as outputs:
                outputs.write(f"release_tag=v{info['version']}\n")
                outputs.write(f"artifact_name={info['package_name']}-{info['version']}\n")
                outputs.write(f"archive_name={info['archive_name']}\n")
                # Build metadata may contain '-' without being a prerelease.
                prerelease = "-" in info["version"].split("+", 1)[0]
                outputs.write(f"prerelease={str(prerelease).lower()}\n")
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
                summary.write(f"### {info['package_name']}\n\nVersion: `{info['version']}`\n\n")
                summary.write(f"Source commit: `{info['commit']}`\n\n[Build record]({info['run_url']})\n")
        print(info["version"])
    else:
        info = json.loads(args.metadata.read_text(encoding="utf-8"))
        print(package(args.root, info, args.output_dir))


if __name__ == "__main__":
    main()
