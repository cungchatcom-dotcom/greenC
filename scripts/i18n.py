"""Translation catalog tools for Green C:\\.

    uv run python scripts/i18n.py extract   # collect tr()/N_() strings -> greenc/i18n/messages.json
    uv run python scripts/i18n.py check     # report missing/extra keys and placeholder mismatches
    uv run python scripts/i18n.py prune     # drop translations whose source string no longer exists

Workflow when UI text changes: edit the Vietnamese string in code, run ``extract``,
translate the new keys in every ``locales/<code>.json`` (``check`` lists them), commit.
"""

from __future__ import annotations

import ast
import json
import string
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "greenc"
I18N_DIR = PACKAGE / "i18n"
MESSAGES = I18N_DIR / "messages.json"
LOCALES = I18N_DIR / "locales"
SOURCES = [PACKAGE, ROOT / "main.py"]
MARKERS = {"tr", "N_"}


def _python_files():
    for src in SOURCES:
        if src.is_file():
            yield src
        else:
            yield from sorted(p for p in src.rglob("*.py") if "i18n" not in p.parts)


def extract_strings(files=None) -> dict[str, list[str]]:
    """Return {source string: [file:line, ...]} for every literal tr()/N_() call."""
    found: dict[str, list[str]] = {}
    for path in files or _python_files():
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            if not (isinstance(node, ast.Call) and node.args):
                continue
            func = node.func
            name = func.id if isinstance(func, ast.Name) else func.attr if isinstance(func, ast.Attribute) else None
            if name not in MARKERS:
                continue
            arg = node.args[0]
            if isinstance(arg, ast.Constant) and isinstance(arg.value, str) and arg.value.strip():
                rel = path.relative_to(ROOT).as_posix()
                found.setdefault(arg.value, []).append(f"{rel}:{node.lineno}")
    return dict(sorted(found.items()))


def placeholders(text: str) -> set[str]:
    try:
        return {field for _, field, _, _ in string.Formatter().parse(text) if field is not None}
    except ValueError:
        return {"<invalid format string>"}


def load_messages() -> list[str]:
    return json.loads(MESSAGES.read_text(encoding="utf-8"))


def check_locale(messages: list[str], catalog: dict[str, str]) -> dict[str, list[str]]:
    keys = set(messages)
    return {
        "missing": [m for m in messages if not catalog.get(m)],
        "extra": sorted(set(catalog) - keys),
        "placeholders": [m for m in messages if catalog.get(m) and placeholders(catalog[m]) != placeholders(m)],
    }


def main(argv: list[str]) -> int:
    command = argv[0] if argv else "check"
    if command == "extract":
        found = extract_strings()
        MESSAGES.write_text(json.dumps(list(found), ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"{len(found)} strings -> {MESSAGES.relative_to(ROOT)}")
        return 0
    messages = load_messages()
    failed = False
    for path in sorted(LOCALES.glob("*.json")):
        catalog = json.loads(path.read_text(encoding="utf-8"))
        if command == "prune":
            kept = {m: catalog[m] for m in messages if m in catalog}
            path.write_text(json.dumps(kept, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
            continue
        report = check_locale(messages, catalog)
        bad = {k: v for k, v in report.items() if v}
        if bad:
            failed = True
            print(f"{path.stem}: " + ", ".join(f"{k}={len(v)}" for k, v in bad.items()))
    if command == "check":
        print("FAIL" if failed else f"OK: {len(list(LOCALES.glob('*.json')))} locales x {len(messages)} strings")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
