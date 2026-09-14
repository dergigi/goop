"""Release policy regressions; no Git mutations or network requests."""
import importlib.util
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("check_release", Path(__file__).with_name("check-release.py"))
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


def changelog(version="2.0.0", date="2026-01-01", category="Added"):
    return f"# Changelog\n\n## [Unreleased]\n\n## [{version}] - {date}\n\n### {category}\n\n- A useful change.\n\n[Unreleased]: https://example.com/compare\n"


class ReleaseTests(unittest.TestCase):
    def test_semver_accepts_stable_prerelease_and_build_metadata(self):
        for version in ["0.0.0", "2.0.0", "2.0.0-rc.1", "2.0.0-1.alpha+build.001"]:
            with self.subTest(version=version):
                self.assertIsNotNone(policy.SEMVER.fullmatch(version))

    def test_semver_rejects_invalid_versions(self):
        for version in ["v2.0.0", "2.0", "02.0.0", "2.0.0-01", "2.0.0-rc..1", "2.0.0+", "2.0.0\n"]:
            with self.subTest(version=version):
                self.assertIsNone(policy.SEMVER.fullmatch(version))

    def test_semver_precedence_is_numeric_and_ignores_build_metadata(self):
        versions = ["1.1.0", "1.2.0-rc.2", "1.2.0-rc.10", "1.2.0", "1.10.0", "2.0.0"]
        self.assertEqual(sorted(versions, key=policy.precedence), versions)
        self.assertEqual(policy.precedence("2.0.0+one"), policy.precedence("2.0.0+two"))

    def test_extracts_only_requested_release_and_preserves_links(self):
        source = changelog().replace("## [Unreleased]", "## [Unreleased]\n\n### Fixed\n\n- Still in development.")
        notes = policy.release_notes(source, "2.0.0")
        self.assertIn("A useful change.", notes)
        self.assertNotIn("Still in development.", notes)
        self.assertIn("[Unreleased]:", notes)

    def test_rejects_missing_duplicate_undated_or_invalid_entries(self):
        for source in [changelog("1.0.0"), changelog() + changelog(),
                       changelog().replace(" - 2026-01-01", ""),
                       changelog(date="2026-02-30"), changelog(date="9999-01-01"),
                       changelog(category="Git commits"),
                       changelog().replace("- A useful change.", ""),
                       changelog().replace("## [Unreleased]", "")]:
            with self.subTest(source=source), self.assertRaises(ValueError):
                policy.release_notes(source, "2.0.0")

    def test_cli_checks_manifest_tag_and_marks_prereleases(self):
        script = Path(__file__).with_name("check-release.py").resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "2.0.0-rc.1"\n')
            (root / "CHANGELOG.md").write_text(changelog("2.0.0-rc.1"))
            output = root / "output"
            result = subprocess.run([sys.executable, str(script), "--tag", "v2.0.0-rc.1", "--github-output", str(output)], cwd=root, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("prerelease=true", output.read_text())
            result = subprocess.run([sys.executable, str(script), "--tag", "v2.0.0"], cwd=root, capture_output=True)
            self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
