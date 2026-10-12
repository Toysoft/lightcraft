#!/usr/bin/env python3
"""Generate the offline What's New release history from published GitHub releases."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import tempfile
from urllib.request import Request, urlopen

API = "https://api.github.com/repos/storytold/lightcraft/releases?per_page=100"
OUTPUT = Path(__file__).resolve().parents[1] / "docs/release-history.md"


def fetch_releases():
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "LightCraft-release-history"}
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = "Bearer " + token
    releases = []
    for page in range(1, 101):
        with urlopen(Request(f"{API}&page={page}", headers=headers), timeout=30) as response:
            batch = json.load(response)
        if not isinstance(batch, list):
            raise ValueError("GitHub did not return a release list")
        releases.extend(batch)
        if len(batch) < 100:
            return releases
    raise ValueError("release history exceeds 100 pages")


def render(releases):
    if not isinstance(releases, list):
        raise ValueError("expected a release list")
    published = []
    seen = set()
    for release in releases:
        if not isinstance(release, dict):
            raise ValueError("expected a release object")
        if release.get("draft") or release.get("prerelease"):
            continue
        tag = release.get("tag_name", "")
        if not isinstance(tag, str) or not re.fullmatch(r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", tag):
            raise ValueError(f"unsupported release tag: {tag!r}")
        date = datetime.strptime(release["published_at"], "%Y-%m-%dT%H:%M:%SZ")
        if tag in seen:
            raise ValueError(f"duplicate release: {tag}")
        seen.add(tag)
        published.append((date, tag))
    lines = ["# LightCraft release history", "", "## Release history", ""]
    lines.extend(f"- {tag} — {date:%Y-%m-%d}" for date, tag in sorted(published, reverse=True))
    if not published:
        lines.append("No published releases yet.")
    lines.extend(["", "Publication dates (UTC).", ""])
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="use a saved GitHub release response without network access")
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    try:
        releases = json.loads(args.input.read_text()) if args.input else fetch_releases()
        text = render(releases)
        # Validate everything before replacing the last usable offline snapshot.
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", newline="\n", dir=args.output.parent, delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(text)
            temporary.replace(args.output)
        finally:
            if temporary and temporary.exists():
                temporary.unlink()
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"release history: {error}\n")


if __name__ == "__main__":
    main()
