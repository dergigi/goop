#!/usr/bin/env python3
"""Validate release metadata and extract its Keep a Changelog section."""
import argparse
from datetime import date
from pathlib import Path
import re
import tomllib

# SemVer 2.0.0: numeric prerelease identifiers cannot have leading zeroes.
NUMBER = r"(?:0|[1-9][0-9]*)"
IDENTIFIER = rf"(?:{NUMBER}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER = re.compile(
    rf"{NUMBER}\.{NUMBER}\.{NUMBER}"
    rf"(?:-(?P<pre>{IDENTIFIER}(?:\.{IDENTIFIER})*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
CATEGORIES = {"Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"}


def precedence(version):
    match = SEMVER.fullmatch(version)
    if not match:
        raise ValueError(f"Invalid SemVer version: {version}")
    core = tuple(map(int, re.split(r"[-+]", version)[0].split(".")))
    pre = match["pre"]
    identifiers = tuple((0, int(part)) if part.isdigit() else (1, part)
                        for part in pre.split(".")) if pre else ((2, ""),)
    return core, identifiers


def release_notes(changelog, version):
    if not SEMVER.fullmatch(version):
        raise ValueError(f"Invalid SemVer version: {version}")
    if not re.search(r"^## \[Unreleased\]\s*$", changelog, re.MULTILINE):
        raise ValueError("CHANGELOG.md must retain an Unreleased section")
    headers = list(re.finditer(r"^## (.+)$", changelog, re.MULTILINE))
    matching = [(i, header) for i, header in enumerate(headers)
                if header[1].startswith(f"[{version}]")]
    if len(matching) != 1:
        raise ValueError(f"Expected exactly one changelog section for {version}")
    index, header = matching[0]
    dated = re.fullmatch(rf"\[{re.escape(version)}\] - (\d{{4}}-\d{{2}}-\d{{2}})", header[1])
    if not dated:
        raise ValueError("Release heading must use ## [VERSION] - YYYY-MM-DD")
    released = date.fromisoformat(dated[1])
    if released > date.today():
        raise ValueError("Release date cannot be in the future")
    end = headers[index + 1].start() if index + 1 < len(headers) else len(changelog)
    body = changelog[header.end():end].strip()
    # Keep comparison link definitions out of the extracted body, then append all
    # references so links used inside an entry still resolve on GitHub.
    references = re.findall(r"^\[[^\]]+\]: .+$", changelog, re.MULTILINE)
    body = re.sub(r"^\[[^\]]+\]: .+$", "", body, flags=re.MULTILINE).strip()
    categories = re.findall(r"^### (.+)$", body, re.MULTILINE)
    if not categories or any(category not in CATEGORIES for category in categories):
        raise ValueError("Use Keep a Changelog change categories in the release section")
    if not re.search(r"^- \S", body, re.MULTILINE):
        raise ValueError("Release section must contain change entries")
    return body + "\n\n" + "\n".join(references) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", help="Validate a prepared version before updating Cargo.toml")
    parser.add_argument("--tag", help="Require this Git tag to match the manifest version")
    parser.add_argument("--notes", type=Path, help="Write extracted release notes to this path")
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    manifest = tomllib.loads(Path("Cargo.toml").read_text())
    version = args.version or manifest["workspace"]["package"]["version"]
    if args.tag and args.tag != f"v{version}":
        parser.error(f"Tag {args.tag} does not match version {version}")
    try:
        if args.version and precedence(version) < precedence(manifest["workspace"]["package"]["version"]):
            raise ValueError("Release version cannot precede the current workspace version")
        notes = release_notes(Path("CHANGELOG.md").read_text(), version)
    except ValueError as error:
        parser.error(str(error))
    if args.notes:
        args.notes.write_text(notes)
    if args.github_output:
        with args.github_output.open("a") as output:
            output.write(f"version={version}\ntag=v{version}\n")
            output.write(f"prerelease={'true' if SEMVER.fullmatch(version)['pre'] else 'false'}\n")
    print(f"Validated release {version}")


if __name__ == "__main__":
    main()
