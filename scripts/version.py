"""Version management for Green C:\\.

The version lives in ``diskguard/__init__.py`` (``__version__``) and is mirrored in
``pyproject.toml``. Releases are recorded in ``CHANGELOG.md`` and tagged ``vX.Y.Z``.

Usage (from the repo root):
    uv run python scripts/version.py show
    uv run python scripts/version.py check            # fails if files disagree
    uv run python scripts/version.py bump patch|minor|major|X.Y.Z [--commit] [--tag]

``bump`` rewrites the version in both files, turns the "Unreleased" section of the
changelog into a dated release section, refreshes uv.lock, and optionally commits
and tags the result.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INIT_FILE = ROOT / "diskguard" / "__init__.py"
PYPROJECT = ROOT / "pyproject.toml"
CHANGELOG = ROOT / "CHANGELOG.md"

SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
_INIT_RE = re.compile(r'^__version__ = "([^"]+)"', re.M)
_PYPROJECT_RE = re.compile(r'^(\[project\][^\[]*?^version = ")([^"]+)(")', re.M | re.S)
UNRELEASED = "## [Unreleased]"


def parse(version: str) -> tuple[int, int, int]:
    match = SEMVER.match(version)
    if not match:
        raise ValueError(f"Not a X.Y.Z version: {version!r}")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def next_version(current: str, part: str) -> str:
    major, minor, patch = parse(current)
    if part == "major":
        return f"{major + 1}.0.0"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    if parse(part) <= (major, minor, patch):
        raise ValueError(f"New version {part} must be greater than {current}")
    return part


def read_versions(root: Path = ROOT) -> dict[str, str | None]:
    init = _INIT_RE.search((root / "diskguard" / "__init__.py").read_text(encoding="utf-8"))
    project = _PYPROJECT_RE.search((root / "pyproject.toml").read_text(encoding="utf-8"))
    return {
        "diskguard/__init__.py": init.group(1) if init else None,
        "pyproject.toml": project.group(2) if project else None,
    }


def current_version(root: Path = ROOT) -> str:
    versions = read_versions(root)
    values = set(versions.values())
    if len(values) != 1 or None in values:
        raise SystemExit(f"Version mismatch: {versions}")
    return values.pop()  # type: ignore[return-value]


def release_changelog(text: str, version: str, date: str) -> str:
    """Move the Unreleased notes under a new ``## [version] - date`` heading."""
    if f"## [{version}]" in text:
        raise ValueError(f"CHANGELOG already has a section for {version}")
    if UNRELEASED not in text:
        raise ValueError(f"CHANGELOG has no '{UNRELEASED}' section")
    return text.replace(UNRELEASED, f"{UNRELEASED}\n\n## [{version}] - {date}", 1)


def write_version(version: str, root: Path = ROOT) -> None:
    init = root / "diskguard" / "__init__.py"
    init.write_text(_INIT_RE.sub(f'__version__ = "{version}"', init.read_text(encoding="utf-8"), count=1), encoding="utf-8")
    pyproject = root / "pyproject.toml"
    pyproject.write_text(
        _PYPROJECT_RE.sub(lambda m: f"{m.group(1)}{version}{m.group(3)}", pyproject.read_text(encoding="utf-8"), count=1),
        encoding="utf-8",
    )


def _git(*args: str) -> None:
    subprocess.run(["git", *args], cwd=ROOT, check=True)


def cmd_bump(part: str, commit: bool, tag: bool) -> None:
    old = current_version()
    new = next_version(old, part)
    if tag and not commit:
        raise SystemExit("--tag requires --commit")
    changelog = release_changelog(CHANGELOG.read_text(encoding="utf-8"), new, _dt.date.today().isoformat())
    write_version(new)
    CHANGELOG.write_text(changelog, encoding="utf-8")
    if shutil.which("uv"):
        subprocess.run(["uv", "lock", "--quiet"], cwd=ROOT, check=True)
    print(f"{old} -> {new}")
    if commit:
        _git("add", "diskguard/__init__.py", "pyproject.toml", "CHANGELOG.md", "uv.lock")
        _git("commit", "-m", f"chore: release v{new}")
    if tag:
        _git("tag", "-a", f"v{new}", "-m", f"Green C:\\ v{new}")
        print(f"Tagged v{new}. Push with: git push origin main v{new}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("show")
    sub.add_parser("check")
    bump = sub.add_parser("bump")
    bump.add_argument("part", help="patch, minor, major or an explicit X.Y.Z")
    bump.add_argument("--commit", action="store_true")
    bump.add_argument("--tag", action="store_true")
    args = parser.parse_args(argv)

    if args.command == "show":
        print(current_version())
    elif args.command == "check":
        version = current_version()
        if f"## [{version}]" not in CHANGELOG.read_text(encoding="utf-8"):
            raise SystemExit(f"CHANGELOG.md has no section for {version}")
        print(f"OK {version}")
    else:
        try:
            cmd_bump(args.part, args.commit, args.tag)
        except ValueError as exc:
            raise SystemExit(str(exc)) from exc
    return 0


if __name__ == "__main__":
    sys.exit(main())
