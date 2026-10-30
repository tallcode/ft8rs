"""The homepage must advertise only complete, published stable downloads."""

from copy import deepcopy
from html.parser import HTMLParser
from pathlib import Path
import re
import tempfile
import unittest

from build_site import build, release_values


def published_release(tag="v1.2.3"):
    base = "https://github.com/tallcode/ft8rs/releases"
    return {
        "tag_name": tag,
        "html_url": f"{base}/tag/{tag}",
        "draft": False,
        "prerelease": False,
        "published_at": "2026-10-29T13:08:03Z",
        "assets": [
            {
                "name": name,
                "state": "uploaded",
                "size": 123,
                "browser_download_url": f"{base}/download/{tag}/{name}",
            }
            for name in [
                "ft8rs-windows-x86_64.zip",
                "ft8rs-linux-x86_64.tar.gz",
                "SHA256SUMS.txt",
            ]
        ],
    }


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.links.append(dict(attrs).get("href", ""))


class BuildSiteTests(unittest.TestCase):
    def test_version_and_all_downloads_belong_to_the_same_release(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            release = published_release()
            build(release, output)
            html = (output / "index.html").read_text(encoding="utf-8")
            links = Links()
            links.feed(html)
            self.assertIn("<strong>1.2.3</strong>", html)
            self.assertNotIn("$version", html)
            for asset in release["assets"]:
                self.assertIn(asset["browser_download_url"], links.links)
            self.assertTrue((output / "style.css").is_file())
            self.assertTrue((output / "synthetic.svg").is_file())
            self.assertTrue((output / "recording.svg").is_file())
            self.assertTrue((output / ".nojekyll").is_file())

    def test_both_languages_share_release_and_have_manual_language_links(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            release = published_release()
            build(release, output)
            for language, other in [("zh", "en"), ("en", "zh")]:
                html = (output / language / "index.html").read_text(encoding="utf-8")
                self.assertIn("<strong>1.2.3</strong>", html)
                self.assertIn(f'href="../{other}/"', html)
                self.assertRegex(html, r'href="\.\./style\.css\?v=[0-9a-f]{8}"')
                self.assertNotIn("<script", html)
                self.assertNotIn("$", html)
                for asset in release["assets"]:
                    self.assertIn(asset["browser_download_url"], html)
            self.assertIn('<html lang="zh-CN">', (output / "zh/index.html").read_text())
            self.assertIn('<html lang="en">', (output / "en/index.html").read_text())
            self.assertIn('src="language.js"', (output / "index.html").read_text())
            self.assertTrue((output / "language.js").is_file())

    def test_unpublished_and_prerelease_versions_cannot_replace_existing_page(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            build(published_release(), output)
            previous = (output / "index.html").read_bytes()
            invalid = [
                {"draft": True},
                {"prerelease": True},
                {"published_at": None},
                {"tag_name": "v1.3.0-rc.1"},
                {"tag_name": "v0.0.0-dev.31.g97e82bd12345"},
                {"tag_name": "main"},
            ]
            for fields in invalid:
                with self.subTest(fields=fields), self.assertRaises(ValueError):
                    build({**published_release(), **fields}, output)
                self.assertEqual((output / "index.html").read_bytes(), previous)

    def test_missing_incomplete_or_ambiguous_downloads_block_publication(self):
        for index in range(3):
            for failure in ["missing", "uploading", "empty", "duplicate"]:
                release = published_release()
                if failure == "missing":
                    release["assets"].pop(index)
                elif failure == "uploading":
                    release["assets"][index]["state"] = "starter"
                elif failure == "empty":
                    release["assets"][index]["size"] = 0
                else:
                    release["assets"].append(deepcopy(release["assets"][index]))
                with self.subTest(index=index, failure=failure), self.assertRaises(ValueError):
                    release_values(release)

    def test_wrong_repository_or_version_download_is_rejected(self):
        for field in ["html_url", "browser_download_url"]:
            for old, new in [("tallcode", "other-owner"), ("v1.2.3", "v1.2.2")]:
                release = published_release()
                item = release if field == "html_url" else release["assets"][0]
                item[field] = item[field].replace(old, new)
                with self.subTest(field=field, new=new), self.assertRaises(ValueError):
                    release_values(release)

    def test_build_metadata_is_not_a_prerelease_and_encoded_urls_are_supported(self):
        release = published_release("v1.2.3+build-4")
        release["html_url"] = release["html_url"].replace("+", "%2B")
        self.assertEqual(release_values(release)["version"], "1.2.3+build-4")


if __name__ == "__main__":
    unittest.main()
