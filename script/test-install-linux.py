#!/usr/bin/env python3
"""Exercise the installer without touching a package manager or the network."""
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().with_name("install-linux")
MOCK = r'''
import hashlib, json, os, pathlib, subprocess, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
config = json.loads(os.environ["GOOP_INSTALL_TEST"])
log = pathlib.Path(os.environ["GOOP_INSTALL_LOG"])
with log.open("a") as out: out.write(json.dumps([name] + args) + "\n")
if name == "uname": print("Linux" if args == ["-s"] else config.get("arch", "x86_64"))
elif name == "id": print(config.get("uid", "1000"))
elif name == "curl":
    url = args[-1]
    if config.get("download_failure"): sys.exit(22)
    output = pathlib.Path(args[args.index("--output") + 1])
    asset = "goop-linux-" + ("arm64" if config.get("arch") == "aarch64" else "x64") + ".flatpak"
    if url.endswith("/latest"):
        output.write_text(json.dumps({"tag_name": "v2.8.1", "draft":False, "prerelease":False}))
    elif url.endswith("SHA256SUMS"):
        digest = "0" * 64 if config.get("corrupt") else hashlib.sha256(b"test bundle").hexdigest()
        output.write_text(digest + "  " + asset + "\n")
    elif url.endswith("manifest-template.json"):
        output.write_text(json.dumps({"runtime":"org.freedesktop.Platform","runtime-version":"24.08"}))
    else: output.write_bytes(b"test bundle")
elif name == "sudo": sys.exit(subprocess.run(args).returncode)
elif name == "apt-get" and args[0] == "install":
    command = pathlib.Path(sys.argv[0]).with_name("flatpak")
    command.write_text(pathlib.Path(sys.argv[0]).read_text())
    command.chmod(0o755)
elif name == "flatpak":
    if config.get("runtime_failure") and "flathub" in args and args[0] == "install": sys.exit(1)
'''


class InstallerTests(unittest.TestCase):
    def run_installer(self, **config):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            for name in ("uname", "id", "curl", "flatpak", "sudo", "apt-get"):
                if name == "flatpak" and config.get("fresh"): continue
                command = folder / name
                command.write_text(f"#!{sys.executable}\n" + MOCK)
                command.chmod(0o755)
            (folder / "python3").symlink_to(sys.executable)
            for name in ("bash", "mktemp", "rm"):
                (folder / name).symlink_to(shutil.which(name))
            log = folder / "calls.jsonl"
            env = dict(os.environ, PATH=str(folder),
                       GOOP_INSTALL_TEST=json.dumps(config), GOOP_INSTALL_LOG=str(log))
            result = subprocess.run(["bash", str(INSTALLER)], env=env, capture_output=True, text=True)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            return result, calls

    def test_x64_and_arm64_install_matching_runtime_before_bundle(self):
        for arch, asset, runtime in [("x86_64", "x64", "x86_64"), ("aarch64", "arm64", "aarch64")]:
            with self.subTest(arch=arch):
                result, calls = self.run_installer(arch=arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                installs = [c for c in calls if c[:2] == ["flatpak", "install"]]
                self.assertEqual(len(installs), 2)
                self.assertEqual(installs[0][-1], f"org.freedesktop.Platform/{runtime}/24.08")
                self.assertTrue(installs[1][-1].endswith(f"goop-linux-{asset}.flatpak"))
                for call in installs:
                    self.assertIn("--user", call)
                    self.assertIn("--or-update", call)
                self.assertFalse(Path(installs[1][-1]).exists(), "Temporary downloads must be removed")
                downloads = [c[-1] for c in calls if c[0] == "curl"]
                self.assertTrue(all("/v2.8.1/" in url for url in downloads[1:]))

    def test_fresh_machine_installs_missing_tools_with_sudo(self):
        result, calls = self.run_installer(fresh=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(["sudo", "apt-get", "update"], calls)
        self.assertIn(["sudo", "apt-get", "install", "-y", "flatpak", "ca-certificates"], calls)
        self.assertEqual(len([c for c in calls if c[:2] == ["flatpak", "install"]]), 2)

    def test_checksum_failure_stops_before_flatpak(self):
        result, calls = self.run_installer(corrupt=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertFalse(any(c[0] == "flatpak" for c in calls))

    def test_download_failure_stops_before_flatpak(self):
        result, calls = self.run_installer(download_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[0] == "flatpak" for c in calls))

    def test_runtime_failure_stops_before_app_install(self):
        result, calls = self.run_installer(runtime_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len([c for c in calls if c[:2] == ["flatpak", "install"]]), 1)

    def test_rejects_root_and_unsupported_architecture_before_download(self):
        for config in ({"uid":"0"}, {"arch":"riscv64"}):
            result, calls = self.run_installer(**config)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(any(c[0] == "curl" for c in calls))


if __name__ == "__main__":
    unittest.main()
