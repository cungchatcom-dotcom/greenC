import importlib.util
import os
import struct
from pathlib import Path

import pytest

from diskguard import __version__
from diskguard.config import APP_VERSION

ROOT = Path(__file__).resolve().parents[1]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


version_tool = _load("version_tool", ROOT / "scripts" / "version.py")


def test_version_is_consistent_everywhere():
    assert APP_VERSION == __version__
    assert set(version_tool.read_versions().values()) == {__version__}
    version_tool.parse(__version__)
    assert f"## [{__version__}]" in (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")


@pytest.mark.parametrize(
    ("current", "part", "expected"),
    [("2.0.0", "patch", "2.0.1"), ("2.0.9", "minor", "2.1.0"), ("2.3.4", "major", "3.0.0"), ("2.0.0", "2.5.0", "2.5.0")],
)
def test_next_version(current, part, expected):
    assert version_tool.next_version(current, part) == expected


@pytest.mark.parametrize("part", ["1.9.9", "2.0.0", "2.0", "banana"])
def test_next_version_rejects_bad_input(part):
    with pytest.raises(ValueError):
        version_tool.next_version("2.0.0", part)


def test_release_changelog_moves_unreleased_notes():
    text = "# Changelog\n\n## [Unreleased]\n\n### Sửa\n- lỗi A\n\n## [2.0.0] - 2026-11-12\n"
    out = version_tool.release_changelog(text, "2.0.1", "2026-12-01")
    assert out.index("## [Unreleased]") < out.index("## [2.0.1] - 2026-12-01") < out.index("- lỗi A")
    with pytest.raises(ValueError):
        version_tool.release_changelog(out, "2.0.1", "2026-12-01")


def test_write_version_updates_both_files(tmp_path):
    (tmp_path / "diskguard").mkdir()
    (tmp_path / "diskguard" / "__init__.py").write_text('"""doc"""\n__version__ = "2.0.0"\n', encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "2.0.0"\n\n[tool.other]\nversion = "9.9.9"\n', encoding="utf-8"
    )
    version_tool.write_version("2.1.0", tmp_path)
    assert version_tool.read_versions(tmp_path) == {"diskguard/__init__.py": "2.1.0", "pyproject.toml": "2.1.0"}
    assert 'version = "9.9.9"' in (tmp_path / "pyproject.toml").read_text(encoding="utf-8")


def test_packaging_icon_and_version_resource():
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    prepare = _load("prepare", ROOT / "packaging" / "prepare.py")
    ico = prepare.build_ico([(16, prepare.png_bytes(16)), (256, prepare.png_bytes(256))])
    reserved, kind, count = struct.unpack("<HHH", ico[:6])
    assert (reserved, kind, count) == (0, 1, 2)
    assert ico[6 + 32 :].startswith(b"\x89PNG")
    info = prepare.version_info("2.3.4")
    assert "filevers=(2, 3, 4, 0)" in info and "'ProductVersion', '2.3.4'" in info


def test_installer_script_matches_app():
    iss = (ROOT / "packaging" / "greenc.iss").read_text(encoding="utf-8-sig")
    main_src = (ROOT / "main.py").read_text(encoding="utf-8")
    assert 'SINGLE_INSTANCE_MUTEX = "GreenCAppMutex"' in main_src
    assert "AppMutex=GreenCAppMutex" in iss
    assert "PrivilegesRequired=lowest" in iss


def test_single_instance_mutex():
    import uuid

    from main import acquire_single_instance

    name = f"GreenCTest-{uuid.uuid4()}"
    first = acquire_single_instance(name)
    assert first is not None
    assert acquire_single_instance(name) is None
