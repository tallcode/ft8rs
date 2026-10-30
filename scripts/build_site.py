#!/usr/bin/env python3
"""Render the static homepage from one published GitHub release (no browser API)."""

import argparse
from datetime import datetime, timezone
import hashlib
from html import escape
import json
from pathlib import Path
import shutil
from string import Template
from urllib.parse import unquote, urlsplit

from package_release import SEMVER


SITE = Path(__file__).resolve().parents[1] / "site"


def checked_url(url, path):
    parsed = urlsplit(url)
    if (
        parsed.scheme != "https"
        or parsed.netloc != "github.com"
        or unquote(parsed.path) != f"/tallcode/ft8rs/{path}"
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(f"unexpected release URL: {url}")
    return url


def release_values(release):
    tag = release["tag_name"]
    if not tag.startswith("v") or not SEMVER.fullmatch(tag[1:]):
        raise ValueError("homepage requires a v-prefixed SemVer release")
    if release["draft"] or release["prerelease"] or not release["published_at"]:
        raise ValueError("homepage requires a successfully published stable release")
    if "-" in tag[1:].split("+", 1)[0]:
        raise ValueError("prerelease versions must not update the homepage")
    release_url = checked_url(release["html_url"], f"releases/tag/{tag}")

    def asset_url(name):
        matches = [asset for asset in release["assets"] if asset["name"] == name]
        if len(matches) != 1 or matches[0]["state"] != "uploaded" or matches[0]["size"] <= 0:
            raise ValueError(f"missing or incomplete release asset: {name}")
        return checked_url(matches[0]["browser_download_url"], f"releases/download/{tag}/{name}")

    published = datetime.fromisoformat(release["published_at"].replace("Z", "+00:00"))
    return {
        "version": tag[1:],
        "published_at": release["published_at"],
        "published_date": published.astimezone(timezone.utc).strftime("%Y-%m-%d %H:%M"),
        "release_url": release_url,
        "windows_url": asset_url("ft8rs-windows-x86_64.zip"),
        "linux_url": asset_url("ft8rs-linux-x86_64.tar.gz"),
        "checksums_url": asset_url("SHA256SUMS.txt"),
    }


def build(release, output):
    values = {key: escape(value, quote=True) for key, value in release_values(release).items()}
    values["style_version"] = hashlib.sha256((SITE / "style.css").read_bytes()).hexdigest()[:8]
    templates = {
        language: Template((SITE / f"{language}.html").read_text(encoding="utf-8"))
        for language in ("zh", "en")
    }
    output.mkdir(parents=True, exist_ok=True)
    for language, template in templates.items():
        directory = output / language
        directory.mkdir(exist_ok=True)
        html = template.substitute(values, asset_prefix="../", language_prefix="../")
        (directory / "index.html").write_text(html, encoding="utf-8")
    # Full English fallback remains usable when JavaScript is disabled. Only the
    # root entry redirects; explicit language URLs never redirect visitors back.
    fallback = templates["en"].substitute(values, asset_prefix="", language_prefix="")
    fallback = fallback.replace("</head>", '  <script src="language.js" defer></script>\n  </head>')
    (output / "index.html").write_text(fallback, encoding="utf-8")
    shutil.copy2(SITE / "style.css", output / "style.css")
    shutil.copy2(SITE / "language.js", output / "language.js")
    shutil.copy2(SITE / "synthetic.svg", output / "synthetic.svg")
    shutil.copy2(SITE / "recording.svg", output / "recording.svg")
    (output / ".nojekyll").touch()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True, help="GitHub REST release JSON")
    parser.add_argument("--tag", required=True, help="Expected tag from the successful release job")
    parser.add_argument("--output", type=Path, default=Path("dist/site"))
    args = parser.parse_args()
    release = json.loads(args.release.read_text(encoding="utf-8"))
    if release["tag_name"] != args.tag:
        parser.error("release metadata does not match the requested tag")
    build(release, args.output)
    print(f"Homepage built at {args.output}")


if __name__ == "__main__":
    main()
