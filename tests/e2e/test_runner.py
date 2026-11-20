#!/usr/bin/env python3
"""
GreenC v2 Opaque-Box E2E Test Suite Master Runner.
Discovers and executes Tier 1 (Feature Coverage), Tier 2 (Boundary & Stress),
Tier 3 (Cross-Feature Combinations), and Tier 4 (Real-World Scenarios) test suites.
Generates structured Markdown, JSON, and JUnit XML reports.
"""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import sys
import time
import unittest
from xml.sax.saxutils import escape

# Ensure workspace root is in sys.path
WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
if str(WORKSPACE_ROOT) not in sys.path:
    sys.path.insert(0, str(WORKSPACE_ROOT))

E2E_DIR = WORKSPACE_ROOT / "tests" / "e2e"


def _engine_description() -> str:
    """States whether the suite ran against the real CLI binary or the reference mock."""
    from tests.e2e.harness import GreenCTestHarness

    h = GreenCTestHarness()
    if h.binary_path and h.binary_path.exists():
        return f"real binary `{h.binary_path}`"
    return "reference MockCliEngine (NOT the real application)"


class CustomTestResult(unittest.TestResult):
    """Collects detailed per-test execution telemetry."""

    def __init__(self):
        super().__init__()
        self.test_records = []
        self.skip_records = []
        self._current_start = 0

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self.skip_records.append({"name": test.id(), "reason": reason})

    def startTest(self, test):
        super().startTest(test)
        self._current_start = time.time()

    def addSuccess(self, test):
        super().addSuccess(test)
        elapsed = (time.time() - self._current_start) * 1000
        self.test_records.append({
            "name": test.id(),
            "status": "PASS",
            "elapsed_ms": elapsed,
            "error": None
        })

    def addFailure(self, test, err):
        super().addFailure(test, err)
        elapsed = (time.time() - self._current_start) * 1000
        self.test_records.append({
            "name": test.id(),
            "status": "FAIL",
            "elapsed_ms": elapsed,
            "error": self._exc_info_to_string(err, test)
        })

    def addError(self, test, err):
        super().addError(test, err)
        elapsed = (time.time() - self._current_start) * 1000
        self.test_records.append({
            "name": test.id(),
            "status": "ERROR",
            "elapsed_ms": elapsed,
            "error": self._exc_info_to_string(err, test)
        })


def discover_suite(tiers: list[int], feature_filter: str = None) -> unittest.TestSuite:
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()

    tier_dirs = {
        1: E2E_DIR / "tier1_features",
        2: E2E_DIR / "tier2_boundary",
        3: E2E_DIR / "tier3_combinations",
        4: E2E_DIR / "tier4_scenarios",
    }

    for t in tiers:
        td = tier_dirs.get(t)
        if td and td.exists():
            discovered = loader.discover(str(td), pattern="test_*.py")
            if feature_filter:
                # Filter test cases by feature name
                filtered = unittest.TestSuite()
                for test_group in discovered:
                    for test_case in test_group:
                        if feature_filter.lower() in str(test_case).lower():
                            filtered.addTest(test_case)
                suite.addTest(filtered)
            else:
                suite.addTest(discovered)

    return suite


def generate_markdown_report(result: CustomTestResult, total_elapsed_s: float, out_path: Path):
    passed = len([r for r in result.test_records if r["status"] == "PASS"])
    failed = len([r for r in result.test_records if r["status"] in ["FAIL", "ERROR"]])
    total = len(result.test_records)
    pass_pct = (passed / total * 100) if total > 0 else 0.0

    lines = [
        "# GreenC v2 E2E Test Execution Report",
        "",
        f"- **Generated**: {datetime.utcnow().isoformat()}Z",
        f"- **Engine**: {_engine_description()}",
        f"- **Total Tests Executed**: {total}",
        f"- **Passed**: {passed} ({pass_pct:.1f}%)",
        f"- **Failed / Errors**: {failed}",
        f"- **Skipped (not counted above)**: {len(result.skip_records)}"
        f" ({sum(1 for r in result.skip_records if r['reason'].startswith('placeholder'))} placeholders"
        f" without real assertions, the rest environment limits)",
        f"- **Total Duration**: {total_elapsed_s:.2f} seconds",
        "",
        "## Summary by Tier",
        "",
        "| Tier | Category | Tests | Passed | Failed |",
        "|------|----------|-------|--------|--------|",
    ]

    tier_map = [
        (("tier1_features", "test_t1_"), "Tier 1", "Feature Coverage (F01–F54)"),
        (("tier2_boundary", "test_t2_"), "Tier 2", "Boundary & Stress Conditions"),
        (("tier3_combinations", "test_t3_"), "Tier 3", "Cross-Feature Combinations"),
        (("tier4_scenarios", "test_t4_"), "Tier 4", "Real-World Scenarios"),
    ]

    for prefixes, t_name, t_desc in tier_map:
        t_records = [r for r in result.test_records if any(p in r["name"] for p in prefixes)]
        t_pass = len([r for r in t_records if r["status"] == "PASS"])
        t_fail = len([r for r in t_records if r["status"] in ["FAIL", "ERROR"]])
        lines.append(f"| {t_name} | {t_desc} | {len(t_records)} | {t_pass} | {t_fail} |")

    lines.extend([
        "",
        "## Failed Tests Detail",
        ""
    ])

    env_skips = [r for r in result.skip_records if not r["reason"].startswith("placeholder")]
    if env_skips:
        lines.extend(["## Skipped for environment reasons", ""])
        for r in env_skips:
            lines.append(f"- `{r['name'].split('.')[-1]}`: {r['reason']}")
        lines.append("")

    failures = [r for r in result.test_records if r["status"] in ["FAIL", "ERROR"]]
    if not failures:
        lines.append("🎉 All test cases passed with zero discrepancies!")
    else:
        for f in failures:
            lines.append(f"### ❌ `{f['name']}` ({f['elapsed_ms']:.1f} ms)")
            lines.append("```")
            lines.append(f['error'] or "Unknown failure")
            lines.append("```")
            lines.append("")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


def generate_junit_xml(result: CustomTestResult, total_elapsed_s: float, out_path: Path):
    passed = len([r for r in result.test_records if r["status"] == "PASS"])
    failed = len([r for r in result.test_records if r["status"] in ["FAIL", "ERROR"]])
    total = len(result.test_records)

    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        f'<testsuites time="{total_elapsed_s:.3f}">',
        f'  <testsuite name="greenc_v2_e2e" tests="{total}" failures="{failed}" errors="0" time="{total_elapsed_s:.3f}">'
    ]

    for r in result.test_records:
        name = escape(r["name"])
        dur = r["elapsed_ms"] / 1000.0
        if r["status"] == "PASS":
            lines.append(f'    <testcase classname="e2e" name="{name}" time="{dur:.3f}" />')
        else:
            err = escape(r["error"] or "Failed")
            lines.append(f'    <testcase classname="e2e" name="{name}" time="{dur:.3f}">')
            lines.append(f'      <failure message="Test failed">{err}</failure>')
            lines.append('    </testcase>')

    lines.extend([
        '  </testsuite>',
        '</testsuites>'
    ])

    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description="GreenC v2 Opaque-Box E2E Test Suite Runner")
    parser.add_argument("--all", action="store_true", help="Run all test tiers (1, 2, 3, 4)")
    parser.add_argument("--tier", type=int, choices=[1, 2, 3, 4], help="Run a specific test tier")
    parser.add_argument("--feature", type=str, help="Filter tests by feature identifier (e.g., F01, F19)")
    parser.add_argument("--report-md", type=str, default="test_report.md", help="Path to write Markdown report")
    parser.add_argument("--report-xml", type=str, help="Path to write JUnit XML report")
    parser.add_argument("--report-json", type=str, help="Path to write JSON summary")
    parser.add_argument("-v", "--verbose", action="store_true", help="Verbose console output")

    args = parser.parse_args()

    selected_tiers = [1, 2, 3, 4] if (args.all or not args.tier) else [args.tier]

    print("================================================================================")
    print("  GreenC v2 Complete Platform — Opaque-Box E2E Test Suite")
    print(f"  Target Workspace: {WORKSPACE_ROOT}")
    print(f"  Selected Tiers: {selected_tiers} | Feature Filter: {args.feature or 'None'}")
    print("================================================================================")

    suite = discover_suite(selected_tiers, args.feature)
    test_count = suite.countTestCases()
    print(f"Discovered {test_count} test cases.")

    result = CustomTestResult()
    start_time = time.time()
    suite.run(result)
    total_elapsed = time.time() - start_time

    passed = len([r for r in result.test_records if r["status"] == "PASS"])
    failed = len([r for r in result.test_records if r["status"] in ["FAIL", "ERROR"]])

    print("--------------------------------------------------------------------------------")
    print(f"Executed: {len(result.test_records)} | Passed: {passed} | Failed: {failed} | "
          f"Skipped: {len(result.skip_records)} | Time: {total_elapsed:.2f}s")
    print("--------------------------------------------------------------------------------")

    if args.report_md:
        md_path = Path(args.report_md)
        generate_markdown_report(result, total_elapsed, md_path)
        print(f"Markdown report generated: {md_path}")

    if args.report_xml:
        xml_path = Path(args.report_xml)
        generate_junit_xml(result, total_elapsed, xml_path)
        print(f"JUnit XML report generated: {xml_path}")

    if args.report_json:
        json_path = Path(args.report_json)
        summary = {
            "timestamp": datetime.utcnow().isoformat() + "Z",
            "total": len(result.test_records),
            "passed": passed,
            "failed": failed,
            "elapsed_seconds": total_elapsed,
            "records": result.test_records
        }
        json_path.parent.mkdir(parents=True, exist_ok=True)
        with open(json_path, "w", encoding="utf-8") as f:
            json.dump(summary, f, indent=2)
        print(f"JSON summary generated: {json_path}")

    if failed > 0:
        print("❌ Test suite finished with failures.")
        sys.exit(1)
    else:
        print("✅ All test cases passed successfully!")
        sys.exit(0)


if __name__ == "__main__":
    main()
