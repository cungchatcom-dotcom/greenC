"""Pytest configuration and shared fixtures for GreenC v2 E2E tests."""

from pathlib import Path
import sys
import pytest

WORKSPACE_ROOT = Path("C:/DiskGuard").resolve()
if str(WORKSPACE_ROOT) not in sys.path:
    sys.path.insert(0, str(WORKSPACE_ROOT))

from tests.e2e.harness import GreenCTestHarness


@pytest.fixture(scope="session")
def harness():
    h = GreenCTestHarness()
    yield h
    h.cleanup()


@pytest.fixture
def sandbox(harness):
    sb = harness.create_sandbox()
    yield sb
