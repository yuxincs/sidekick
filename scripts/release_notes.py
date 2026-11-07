#!/usr/bin/env python3
"""Generate release notes from commits since the nearest reachable version tag."""

from __future__ import annotations

import argparse
import html
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import quote

VERSION_TAG = re.compile(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?")


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], text=True).strip()


def release_notes(tag: str, repository: str) -> str:
    if not VERSION_TAG.fullmatch(tag):
        raise ValueError("Release tags must use vMAJOR.MINOR.PATCH, optionally with a prerelease/build suffix")
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    if tag != f"v{version}":
        raise ValueError(f"Tag {tag} does not match Cargo.toml version {version}")
    if git("rev-parse", f"{tag}^{{commit}}") != git("rev-parse", "HEAD"):
        raise ValueError("Check out the release tag before generating its notes")
    tags = [
        candidate
        for candidate in git("tag", "--merged", tag).splitlines()
        if candidate != tag and VERSION_TAG.fullmatch(candidate)
    ]
    previous = (
        git("describe", "--tags", "--abbrev=0", *[f"--match={candidate}" for candidate in tags], tag) if tags else None
    )
    revision = f"{previous}..{tag}" if previous else tag
    commits = git("log", "--reverse", "--format=%H%x00%s", revision)
    base = f"https://github.com/{repository}"
    lines = ["## Changes", ""]
    if previous:
        lines += [f"Commits since [{previous}]({base}/releases/tag/{quote(previous, safe='')}):", ""]
    else:
        lines += ["Initial release, including all commits up to this version.", ""]
    for commit in commits.splitlines():
        sha, subject = commit.split("\x00", 1)
        subject = re.sub(r"([\\`*_{}\[\]()#+!|])", r"\\\1", html.escape(subject)).replace("@", "&#64;")
        lines.append(f"- {subject} ([{sha[:7]}]({base}/commit/{sha}))")
    if not commits:
        lines.append("No new commits since the previous version.")
    if previous:
        lines += ["", f"[Full changelog]({base}/compare/{quote(previous, safe='')}...{quote(tag, safe='')})"]
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(release_notes(args.tag, args.repository))


if __name__ == "__main__":
    main()
