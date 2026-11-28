"""Exercise the release workflow's native cache keys and planner."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent
ARCHIVE = "mekugi_linux_amd64-example"
RECEIPT = "native-tests-linux_amd64-example"
REPO = "example/mekugi"

FAKE_GH = r"""
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
if args[0] == "run":
    if args[2] != "3":
        sys.exit(1)
    archive_dir = Path(args[args.index("--dir") + 1])
    archive_dir.mkdir()
    (archive_dir / "mekugi-native_linux_amd64.tar.gz").write_bytes(b"native")
elif args[1] == "--paginate":
    artifact = args[args.index("-f") + 1].removeprefix("name=")
    print(*json.loads(os.environ["TEST_ARTIFACTS"]).get(artifact, []), sep="\n")
else:
    print(json.loads(os.environ["TEST_RUNS"])[args[-1].split("/")[-1]])
"""


class NativeCacheTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.output = self.root / "output"
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_")
        } | {
            "ARCHIVE_CACHE_HIT": "",
            "ARCHIVE_NAME": ARCHIVE,
            "GITHUB_OUTPUT": str(self.output),
            "GITHUB_REPOSITORY": REPO,
            "GITHUB_RUN_ATTEMPT": "1",
            "GITHUB_RUN_ID": "42",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "NATIVE_BUILD_IMAGE": "ci-image",
            "NATIVE_TEST_IMAGE": "sha256:image-one",
            "NATIVE_PACKAGE_PROFILE": "dev-small",
            "PATH": os.pathsep.join((str(self.root), os.environ["PATH"])),
            "PLATFORM": "linux_amd64",
            "RUNNER_LABEL": "ubuntu-24.04",
            "RUNNER_TEMP": str(self.root),
            "TEST_ARTIFACTS": "{}",
            "TEST_CACHE_HIT": "",
            "TEST_RECEIPT": RECEIPT,
            "TEST_RUNS": "{}",
        }
        self.env.pop("ImageVersion", None)

    def run_command(self, *args, **env):
        subprocess.run(
            args, env=self.env | env, cwd=self.root, check=True, stdout=subprocess.PIPE
        )

    def commit(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        self.run_command("git", "add", name)
        self.run_command("git", "commit", "--quiet", "-m", "fixture")

    def outputs(self, *args, **env):
        self.output.unlink(missing_ok=True)
        self.run_command(*args, **env)
        return dict(line.split("=", 1) for line in self.output.read_text().splitlines())

    def keys(self, **env):
        return self.outputs(sys.executable, str(SCRIPTS / "native-keys.py"), **env)

    def test_keys_follow_package_and_test_inputs(self):
        self.run_command("git", "init", "--quiet")
        self.run_command("git", "config", "user.email", "ci@example.com")
        self.run_command("git", "config", "user.name", "Native cache test")
        self.commit("codex-rs/core/src/lib.rs", "production")
        original = self.keys()
        for path in (
            "mekugi-tui/main.go",
            "codex-rs/app-server/tests/suite.rs",
            "codex-rs/.config/nextest.toml",
            "codex-rs/app-server/src/bin/exec_server.rs",
            "codex-rs/core/src/lib_tests.rs",
            "codex-rs/app-server-subprocess-tests/src/lib.rs",
            ".github/workflows/release.yml",
        ):
            with self.subTest(path=path):
                before = self.keys()
                self.commit(path, "fixture")
                after = self.keys()
                self.assertEqual(original["archive"], after["archive"])
                self.assertEqual(
                    before["tests"] == after["tests"], path == "mekugi-tui/main.go"
                )

        for path in (
            "codex-rs/core/src/lib.rs",
            "codex-rs/app-server-protocol/schema/example.json",
            "codex-rs/app-server-subprocess-tests/Cargo.toml",
            "third_party/v8/pin.sha256",
            ".github/scripts/native-cargo.sh",
            ".github/ci-image/Dockerfile",
        ):
            with self.subTest(path=path):
                before = self.keys()
                self.commit(path, "changed")
                after = self.keys()
                self.assertNotEqual(before["archive"], after["archive"])
                self.assertNotEqual(before["tests"], after["tests"])

    def test_platform_and_release_identity(self):
        self.run_command("git", "init", "--quiet")
        self.run_command("git", "config", "user.email", "ci@example.com")
        self.run_command("git", "config", "user.name", "Native cache test")
        self.commit("LICENSE", "fixture")
        dev = self.keys()
        self.assertNotEqual(
            dev["archive"], self.keys(PLATFORM="linux_arm64")["archive"]
        )
        self.assertEqual(
            dev, self.keys(ImageVersion="new-image", GITHUB_RUN_ATTEMPT="2")
        )

        changed_image = self.keys(NATIVE_TEST_IMAGE="sha256:image-two")
        self.assertEqual(dev["archive"], changed_image["archive"])
        self.assertNotEqual(dev["tests"], changed_image["tests"])
        unresolved_image = self.keys(NATIVE_TEST_IMAGE="")
        retry = self.keys(NATIVE_TEST_IMAGE="", GITHUB_RUN_ATTEMPT="2")
        self.assertEqual(unresolved_image["archive"], retry["archive"])
        self.assertNotEqual(unresolved_image["tests"], retry["tests"])

        darwin = self.keys(PLATFORM="darwin_arm64", ImageVersion="macos-one")
        changed_darwin = self.keys(PLATFORM="darwin_arm64", ImageVersion="macos-two")
        self.assertEqual(darwin["archive"], changed_darwin["archive"])
        self.assertNotEqual(darwin["tests"], changed_darwin["tests"])

        release = self.keys(NATIVE_PACKAGE_PROFILE="release")
        self.assertNotEqual(dev["archive"], release["archive"])
        self.assertNotEqual(
            release["archive"],
            self.keys(NATIVE_PACKAGE_PROFILE="release", GITHUB_RUN_ATTEMPT="2")[
                "archive"
            ],
        )

    def prepare_gh(self, artifacts, runs):
        script = self.root / "gh"
        script.write_text("#!/usr/bin/env python3\n" + FAKE_GH)
        script.chmod(0o755)
        self.env.update(
            TEST_ARTIFACTS=json.dumps(artifacts), TEST_RUNS=json.dumps(runs)
        )

    @staticmethod
    def source(event="push", branch="main", repo=REPO):
        return json.dumps(
            {
                "conclusion": "success",
                "event": event,
                "head_branch": branch,
                "head_repository": {"full_name": repo},
                "path": ".github/workflows/release.yml",
                "run_attempt": 1,
            }
        )

    def plan(self, **env):
        return self.outputs("bash", str(SCRIPTS / "plan-native.sh"), **env)

    def test_package_and_test_decisions_are_independent(self):
        self.prepare_gh({}, {})
        self.assertEqual(self.plan(), {"test": "true", "package": "true"})
        self.assertEqual(
            self.plan(ARCHIVE_CACHE_HIT="true"), {"test": "true", "package": "false"}
        )
        (self.root / "native-tests-passed").write_text(RECEIPT + "\n")
        self.assertEqual(
            self.plan(TEST_CACHE_HIT="true"), {"test": "false", "package": "true"}
        )
        self.assertEqual(
            self.plan(ARCHIVE_CACHE_HIT="true", TEST_CACHE_HIT="true"),
            {"test": "false", "package": "false"},
        )
        self.assertEqual(
            self.plan(
                ARCHIVE_CACHE_HIT="true",
                TEST_CACHE_HIT="true",
                NATIVE_PACKAGE_PROFILE="release",
            ),
            {"test": "true", "package": "false"},
        )

    def test_only_trusted_matching_run_artifacts_are_reused(self):
        self.prepare_gh(
            {ARCHIVE: [1, 2, 3], RECEIPT: [1, 4], "native-approved-run-3": [4]},
            {
                "1": self.source(repo="fork/mekugi"),
                "2": self.source(event="pull_request"),
                "3": self.source(event="pull_request"),
                "4": self.source(),
            },
        )
        self.assertEqual(
            self.plan(TEST_ARTIFACTS=json.dumps({ARCHIVE: [1, 2], RECEIPT: [1, 2]})),
            {"test": "true", "package": "true"},
        )
        self.assertEqual(self.plan(), {"test": "false", "package": "false"})
        self.assertEqual(
            (self.root / "native-tests-passed").read_text(), RECEIPT + "\n"
        )
        self.assertEqual(
            (self.root / "artifacts/mekugi-native_linux_amd64.tar.gz").read_bytes(),
            b"native",
        )


if __name__ == "__main__":
    unittest.main()
