"""Unit tests for path-scoped verification selection in scripts/affected.py."""

from __future__ import annotations

import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]


def load_affected():
    spec = importlib.util.spec_from_file_location(
        "affected", ROOT / "scripts/affected.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["affected"] = module
    spec.loader.exec_module(module)
    return module


AFFECTED = load_affected()


def make_fixture_repo(root: Path) -> None:
    (root / "mekugi-webui/app/src").mkdir(parents=True)
    (root / "mekugi-webui/app/src/App.svelte").write_text("<div/>\n")
    (root / "mekugi-webui/go.mod").write_text("module example.com/webui\n")
    (root / "mekugi-webui/assets.go").write_text("package webui\n")

    (root / "mekugi-tui").mkdir(parents=True)
    (root / "mekugi-tui/go.work").write_text("go 1.27.0\n")
    (root / "mekugi-tui/modules/usage").mkdir(parents=True)
    (root / "mekugi-tui/modules/usage/go.mod").write_text("module example.com/usage\n")
    (root / "mekugi-tui/modules/usage/usage.go").write_text("package usage\n")
    (root / "mekugi-tui/apps/mekugi/internal/launch/testdata").mkdir(parents=True)
    (root / "mekugi-tui/apps/mekugi/go.mod").write_text("module example.com/mekugi\n")
    (root / "mekugi-tui/apps/mekugi/internal/launch/launch.go").write_text(
        "package launch\n"
    )
    (root / "mekugi-tui/apps/mekugi/internal/launch/testdata/snap.txt").write_text(
        "ok\n"
    )

    (root / "codex-rs/core/src").mkdir(parents=True)
    (root / "codex-rs/protocol/src").mkdir(parents=True)
    (root / "codex-rs/tui/src").mkdir(parents=True)
    (root / "codex-rs/Cargo.toml").write_text(
        '[workspace]\nmembers = ["core", "protocol"]\nexclude = ["tui"]\n'
    )
    (root / "codex-rs/Cargo.lock").write_text("# lock\n")
    (root / "codex-rs/core/Cargo.toml").write_text('[package]\nname = "codex-core"\n')
    (root / "codex-rs/core/src/lib.rs").write_text("")
    (root / "codex-rs/protocol/Cargo.toml").write_text(
        '[package]\nname = "codex-protocol"\n'
    )
    (root / "codex-rs/protocol/src/lib.rs").write_text("")
    (root / "codex-rs/tui/Cargo.toml").write_text('[package]\nname = "codex-tui"\n')
    (root / "codex-rs/tui/src/lib.rs").write_text("")


class AffectedPlanTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        make_fixture_repo(self.repo)

    def tearDown(self):
        self._tmp.cleanup()

    def test_webui_only_skips_go_and_cargo(self):
        plan = AFFECTED.plan_affected(
            ["mekugi-webui/app/src/App.svelte", "mekugi-webui/DESIGN.md"],
            self.repo,
        )
        self.assertTrue(plan.webui)
        self.assertEqual(plan.go_packages, ())
        self.assertEqual(plan.rust_packages, ())
        self.assertFalse(plan.rust_fallback)
        self.assertEqual(plan.commands(), [["just", "webui-check"]])

    def test_webui_go_file_runs_webui_and_go_package(self):
        plan = AFFECTED.plan_affected(["mekugi-webui/assets.go"], self.repo)
        self.assertEqual(
            plan.commands(),
            [["just", "webui-check"], ["just", "test", "./mekugi-webui"]],
        )

    def test_tui_scoped_packages_skip_cargo_and_webui(self):
        plan = AFFECTED.plan_affected(
            [
                "mekugi-tui/modules/usage/usage.go",
                "mekugi-tui/apps/mekugi/internal/launch/testdata/snap.txt",
            ],
            self.repo,
        )
        self.assertFalse(plan.webui)
        self.assertEqual(plan.rust_packages, ())
        self.assertFalse(plan.rust_fallback)
        self.assertEqual(
            plan.commands(),
            [
                [
                    "just",
                    "test",
                    "./mekugi-tui/apps/mekugi/internal/launch",
                    "./mekugi-tui/modules/usage",
                ]
            ],
        )

    def test_tui_module_wildcard_prunes_redundant_subpackages(self):
        (self.repo / "mekugi-tui/apps/mekugi/README.md").write_text("# app\n")
        plan = AFFECTED.plan_affected(
            [
                "mekugi-tui/apps/mekugi/README.md",
                "mekugi-tui/apps/mekugi/internal/launch/launch.go",
            ],
            self.repo,
        )
        self.assertEqual(
            plan.commands(),
            [["just", "test", "./mekugi-tui/apps/mekugi/..."]],
        )

    def test_tui_go_work_or_go_mod_falls_back_to_workspace(self):
        for changed in (
            ["mekugi-tui/go.work"],
            ["mekugi-tui/modules/usage/go.mod", "mekugi-tui/modules/usage/usage.go"],
            ["mekugi-tui/README.md"],
        ):
            with self.subTest(changed=changed):
                plan = AFFECTED.plan_affected(changed, self.repo)
                self.assertEqual(plan.commands(), [["just", "test", "@workspace"]])

    def test_rust_single_and_multiple_crates(self):
        single = AFFECTED.plan_affected(["codex-rs/protocol/src/lib.rs"], self.repo)
        self.assertEqual(
            single.commands(),
            [
                [
                    "just",
                    "test-rust",
                    "--locked",
                    "-p",
                    "codex-protocol",
                    "-E",
                    "package(codex-protocol)",
                ]
            ],
        )

        core_only = AFFECTED.plan_affected(["codex-rs/core/src/lib.rs"], self.repo)
        self.assertEqual(
            core_only.commands(),
            [
                [
                    "just",
                    "test-rust",
                    "--locked",
                    "-p",
                    "codex-code-mode-host",
                    "-p",
                    "codex-core",
                    "-E",
                    "package(codex-core)",
                ]
            ],
        )

        multi = AFFECTED.plan_affected(
            ["codex-rs/protocol/src/lib.rs", "codex-rs/core/src/lib.rs"],
            self.repo,
        )
        self.assertEqual(
            multi.commands(),
            [
                [
                    "just",
                    "test-rust",
                    "--locked",
                    "-p",
                    "codex-code-mode-host",
                    "-p",
                    "codex-core",
                    "-p",
                    "codex-protocol",
                    "-E",
                    "package(codex-core) | package(codex-protocol)",
                ]
            ],
        )

    def test_rust_workspace_level_or_excluded_falls_back_safely(self):
        for changed in (
            ["codex-rs/Cargo.lock"],
            ["codex-rs/.config/nextest.toml"],
            ["codex-rs/tui/src/lib.rs"],
            ["codex-rs/scripts/helper.py"],
        ):
            with self.subTest(changed=changed):
                plan = AFFECTED.plan_affected(changed, self.repo)
                self.assertTrue(plan.rust_fallback)
                self.assertEqual(plan.commands(), [["just", "test-native"]])

    def test_rust_workspace_file_with_crate_keeps_scoped_crate_tests(self):
        plan = AFFECTED.plan_affected(
            ["codex-rs/Cargo.lock", "codex-rs/protocol/src/lib.rs"],
            self.repo,
        )
        self.assertTrue(plan.rust_fallback)
        self.assertEqual(
            plan.commands(),
            [
                ["just", "test-native"],
                [
                    "just",
                    "test-rust",
                    "--locked",
                    "-p",
                    "codex-protocol",
                    "-E",
                    "package(codex-protocol)",
                ],
            ],
        )

    def test_mixed_diff_runs_all_affected_targets(self):
        plan = AFFECTED.plan_affected(
            [
                "mekugi-webui/app/src/App.svelte",
                "mekugi-tui/modules/usage/usage.go",
                "codex-rs/core/src/lib.rs",
            ],
            self.repo,
        )
        self.assertEqual(
            plan.commands(),
            [
                ["just", "webui-check"],
                ["just", "test", "./mekugi-tui/modules/usage"],
                [
                    "just",
                    "test-rust",
                    "--locked",
                    "-p",
                    "codex-code-mode-host",
                    "-p",
                    "codex-core",
                    "-E",
                    "package(codex-core)",
                ],
            ],
        )

    def test_unrelated_files_produce_no_commands(self):
        plan = AFFECTED.plan_affected(
            ["README.md", "docs/index.md", "justfile"], self.repo
        )
        self.assertEqual(plan.commands(), [])

    def test_main_no_targets_prints_message_and_exits_zero(self):
        stdout = io.StringIO()
        with (
            patch.object(AFFECTED, "collect_changed_files", return_value=["README.md"]),
            patch.object(AFFECTED, "REPO_ROOT", self.repo),
            patch("sys.stdout", stdout),
        ):
            code = AFFECTED.main(["origin/main"])
        self.assertEqual(code, 0)
        self.assertIn("No affected verification targets", stdout.getvalue())

    def test_main_dry_run_accepts_flag_without_base(self):
        stdout = io.StringIO()
        with (
            patch.object(
                AFFECTED,
                "collect_changed_files",
                return_value=["mekugi-webui/app/src/App.svelte"],
            ) as collect,
            patch.object(AFFECTED, "REPO_ROOT", self.repo),
            patch("sys.stdout", stdout),
        ):
            code = AFFECTED.main(["--dry-run"])
        self.assertEqual(code, 0)
        collect.assert_called_once_with(self.repo, "origin/main")
        self.assertIn("[dry-run] just webui-check", stdout.getvalue())

    def test_real_repo_crate_and_go_package_mapping(self):
        plan = AFFECTED.plan_affected(
            [
                "codex-rs/core/src/lib.rs",
                "codex-rs/app-server/src/main.rs",
                "mekugi-tui/modules/usage/usage.go",
            ],
            ROOT,
        )
        self.assertEqual(
            plan.commands(),
            [
                ["just", "test", "./mekugi-tui/modules/usage"],
                [
                    "just",
                    "test-rust",
                    "--locked",
                    "-p",
                    "codex-app-server",
                    "-p",
                    "codex-code-mode-host",
                    "-p",
                    "codex-core",
                    "-E",
                    "package(codex-app-server) | package(codex-core)",
                ],
            ],
        )

    def test_python310_toml_fallback_parses_package_and_excludes(self):
        (self.repo / "codex-rs/protocol/Cargo.toml").write_text(
            '[package]\n"name" = "codex-protocol"\n\n[[bin]]\nname = "protocol-bin"\n'
        )
        with patch.object(AFFECTED, "tomllib", None):
            plan = AFFECTED.plan_affected(
                ["codex-rs/protocol/src/lib.rs"],
                self.repo,
            )
            self.assertEqual(
                plan.commands(),
                [
                    [
                        "just",
                        "test-rust",
                        "--locked",
                        "-p",
                        "codex-protocol",
                        "-E",
                        "package(codex-protocol)",
                    ]
                ],
            )
            excluded = AFFECTED.plan_affected(["codex-rs/tui/src/lib.rs"], self.repo)
            self.assertTrue(excluded.rust_fallback)
            self.assertIsNone(
                AFFECTED._read_package_name(
                    '[package]\nversion = "0.1.0"\n\n[[bin]]\nname = "only-bin"\n'
                )
            )

    def test_collect_changed_files_catches_committed_staged_unstaged_and_untracked(
        self,
    ):
        def git(*args: str) -> None:
            subprocess.run(
                ["git", *args],
                cwd=self.repo,
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env={
                    "GIT_AUTHOR_NAME": "Test",
                    "GIT_AUTHOR_EMAIL": "test@example.com",
                    "GIT_COMMITTER_NAME": "Test",
                    "GIT_COMMITTER_EMAIL": "test@example.com",
                    "GIT_CONFIG_NOSYSTEM": "1",
                    "GIT_CONFIG_GLOBAL": "/dev/null",
                    "PATH": "/usr/bin:/bin:/usr/local/bin",
                },
            )

        git("init", "-b", "main")
        git("add", ".")
        git("commit", "-m", "base")
        git("checkout", "-b", "feat")

        # Committed change
        (self.repo / "codex-rs/core/src/lib.rs").write_text("// committed\n")
        git("add", "codex-rs/core/src/lib.rs")
        git("commit", "-m", "committed")

        # Staged change
        (self.repo / "mekugi-tui/modules/usage/usage.go").write_text(
            "package usage\n// staged\n"
        )
        git("add", "mekugi-tui/modules/usage/usage.go")

        # Unstaged change
        (self.repo / "mekugi-webui/app/src/App.svelte").write_text("<span>hi</span>\n")

        # Untracked file
        (self.repo / "codex-rs/protocol/src/new_file.rs").write_text("// new\n")

        changed = AFFECTED.collect_changed_files(self.repo, "main")
        self.assertEqual(
            changed,
            [
                "codex-rs/core/src/lib.rs",
                "codex-rs/protocol/src/new_file.rs",
                "mekugi-tui/modules/usage/usage.go",
                "mekugi-webui/app/src/App.svelte",
            ],
        )

    def test_collect_changed_files_reports_both_sides_of_rename(self):
        def git(*args: str) -> None:
            subprocess.run(
                ["git", *args],
                cwd=self.repo,
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env={
                    "GIT_AUTHOR_NAME": "Test",
                    "GIT_AUTHOR_EMAIL": "test@example.com",
                    "GIT_COMMITTER_NAME": "Test",
                    "GIT_COMMITTER_EMAIL": "test@example.com",
                    "GIT_CONFIG_NOSYSTEM": "1",
                    "GIT_CONFIG_GLOBAL": "/dev/null",
                    "PATH": "/usr/bin:/bin:/usr/local/bin",
                },
            )

        (self.repo / "codex-rs/core/src/moved.rs").write_text("pub fn moved() {}\n")
        git("init", "-b", "main")
        git("add", ".")
        git("commit", "-m", "base")
        git("checkout", "-b", "feat")
        git("mv", "codex-rs/core/src/moved.rs", "codex-rs/protocol/src/moved.rs")

        changed = AFFECTED.collect_changed_files(self.repo, "main")
        self.assertEqual(
            changed,
            ["codex-rs/core/src/moved.rs", "codex-rs/protocol/src/moved.rs"],
        )

    def test_protocol_schema_changes_trigger_go_contract_and_webui(self) -> None:
        (self.repo / "codex-rs/app-server-protocol/Cargo.toml").parent.mkdir(parents=True, exist_ok=True)
        (self.repo / "codex-rs/app-server-protocol/Cargo.toml").write_text(
            '[package]\nname = "codex-app-server-protocol"\n'
        )
        plan = AFFECTED.plan_affected(
            [
                "codex-rs/app-server-protocol/schema/json/ClientRequest.json",
                "codex-rs/app-server-protocol/schema/typescript/ClientRequest.ts",
            ],
            self.repo,
        )
        self.assertTrue(plan.webui)
        self.assertIn("./mekugi-tui/apps/mekugi/internal/launch", plan.go_packages)
        self.assertIn("codex-app-server-protocol", plan.rust_packages)

        exp_plan = AFFECTED.plan_affected(
            ["codex-rs/app-server-protocol/src/protocol/common.rs"],
            self.repo,
        )
        self.assertIn("./mekugi-tui/apps/mekugi/internal/launch", exp_plan.go_packages)


if __name__ == "__main__":
    unittest.main()
