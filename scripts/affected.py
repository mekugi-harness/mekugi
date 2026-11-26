#!/usr/bin/env python3
"""Run path-scoped verification targets for files changed since a base ref."""

from __future__ import annotations

import argparse
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path
import re
import shlex
import subprocess
import sys

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 fallback
    tomllib = None  # type: ignore[assignment]


REPO_ROOT = Path(__file__).resolve().parents[1]
GO_WORKSPACE_FILES = frozenset({"go.work", "go.work.sum", "go.mod", "go.sum"})
# Crates whose integration tests spawn companion workspace binaries via cargo_bin().
RUST_COMPANION_PACKAGES: dict[str, tuple[str, ...]] = {
    "codex-core": ("codex-code-mode-host",),
    "codex-app-server": ("codex-code-mode-host",),
    "codex-app-server-subprocess-tests": ("codex-app-server", "codex-code-mode-host"),
}


@dataclass(frozen=True)
class VerificationPlan:
    changed_files: tuple[str, ...]
    webui: bool = False
    go_packages: tuple[str, ...] = ()
    rust_packages: tuple[str, ...] = ()
    rust_fallback: bool = False

    def commands(self, just_bin: str = "just") -> list[list[str]]:
        commands: list[list[str]] = []
        if self.webui:
            commands.append([just_bin, "webui-check"])
        if self.go_packages:
            commands.append([just_bin, "test", *self.go_packages])
        if self.rust_fallback:
            commands.append([just_bin, "test-native"])
        if self.rust_packages:
            build_packages = set(self.rust_packages)
            for pkg in self.rust_packages:
                build_packages.update(RUST_COMPANION_PACKAGES.get(pkg, ()))
            pkg_flags = [arg for pkg in sorted(build_packages) for arg in ("-p", pkg)]
            filter_expr = " | ".join(f"package({pkg})" for pkg in self.rust_packages)
            commands.append(
                [just_bin, "test-rust", "--locked", *pkg_flags, "-E", filter_expr]
            )
        return commands


def _git_nul_paths(args: list[str], repo_root: Path) -> list[str]:
    process = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if process.returncode != 0:
        detail = process.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(
            f"git {' '.join(args)} failed: {detail or process.returncode}"
        )
    raw = process.stdout.decode("utf-8", errors="surrogateescape")
    return [entry for entry in raw.split("\0") if entry]


def collect_changed_files(repo_root: Path, base: str) -> list[str]:
    """Return sorted unique repo-relative paths from base...HEAD and local edits."""
    paths: set[str] = set()
    queries = (
        ["diff", "-z", "--name-only", "--no-renames", f"{base}...HEAD"],
        ["diff", "-z", "--name-only", "--no-renames", "--cached"],
        ["diff", "-z", "--name-only", "--no-renames"],
        ["ls-files", "-z", "--others", "--exclude-standard"],
    )
    for query in queries:
        paths.update(_git_nul_paths(query, repo_root))
    return sorted(paths)


def _normalize_rel_path(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _parse_toml_section(text: str, section: str) -> str:
    """Extract the body of a top-level TOML table for Python 3.10 fallback."""
    header_re = re.compile(r"^\s*(\[\[?)\s*([^\[\]]+?)\s*\]\]?\s*(?:#.*)?$")
    lines: list[str] = []
    in_section = False
    for line in text.splitlines():
        match = header_re.match(line)
        if match:
            in_section = match.group(1) == "[" and match.group(2).strip() == section
            continue
        if in_section:
            lines.append(line)
    return "\n".join(lines)


def _read_workspace_excludes(text: str) -> list[str]:
    if tomllib is not None:
        try:
            data = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            return []
        excluded = data.get("workspace", {}).get("exclude", [])
        return [item for item in excluded if isinstance(item, str)]
    body = _parse_toml_section(text, "workspace")
    match = re.search(r"(?m)^\s*exclude\s*=\s*\[(.*?)\]", body, re.DOTALL)
    if not match:
        return []
    return [a or b for a, b in re.findall(r'"([^"]+)"|\'([^\']+)\'', match.group(1))]


def _read_package_name(text: str) -> str | None:
    if tomllib is not None:
        try:
            data = tomllib.loads(text)
        except tomllib.TOMLDecodeError:
            return None
        package_name = data.get("package", {}).get("name")
        return package_name if isinstance(package_name, str) and package_name else None
    body = _parse_toml_section(text, "package")
    match = re.search(
        r'(?m)^\s*(?:name|"name"|\'name\')\s*=\s*(?:"([^"]+)"|\'([^\']+)\')',
        body,
    )
    if not match:
        return None
    return match.group(1) or match.group(2)


def _load_rust_excluded_dirs(repo_root: Path) -> frozenset[str]:
    manifest = repo_root / "codex-rs" / "Cargo.toml"
    if not manifest.is_file():
        return frozenset()
    try:
        text = manifest.read_text(encoding="utf-8")
    except OSError:
        return frozenset()
    return frozenset(
        item.replace("\\", "/").strip("/") for item in _read_workspace_excludes(text)
    )


def _is_excluded_rust_dir(
    rel_from_codex_rs: Path, excluded_dirs: frozenset[str]
) -> bool:
    parts = rel_from_codex_rs.parts
    for end in range(1, len(parts) + 1):
        if "/".join(parts[:end]) in excluded_dirs:
            return True
    return False


def _dir_has_go_files(directory: Path) -> bool:
    if not directory.is_dir():
        return False
    return any(
        entry.is_file() and entry.name.endswith(".go") for entry in directory.iterdir()
    )


def resolve_go_package(
    rel_path: str, repo_root: Path, root_dir: str = "mekugi-tui"
) -> str | None:
    """Resolve a changed file under a Go tree to its enclosing package pattern."""
    rel = Path(rel_path)
    root = Path(root_dir)
    if rel == root or root not in rel.parents:
        return None
    if rel.name in GO_WORKSPACE_FILES:
        return None
    # Files directly under mekugi-tui/ have no enclosing Go module.
    if root_dir == "mekugi-tui" and rel.parent == root:
        return None

    cur = rel.parent
    while True:
        full_dir = repo_root / cur
        if _dir_has_go_files(full_dir):
            return f"./{cur.as_posix()}"
        if (full_dir / "go.mod").is_file():
            return f"./{cur.as_posix()}/..."
        if cur == root or cur.parent == cur:
            return None
        cur = cur.parent


def resolve_rust_package(
    rel_path: str,
    repo_root: Path,
    excluded_dirs: frozenset[str],
) -> str | None:
    """Resolve a changed file under codex-rs/ to its enclosing Cargo package name."""
    rel = Path(rel_path)
    rust_root = Path("codex-rs")
    if rel == rust_root or rust_root not in rel.parents:
        return None
    if rel.parent == rust_root:
        return None
    # Workspace-level config directories under codex-rs/ affect all crates.
    if len(rel.parts) >= 2 and rel.parts[1] in {".cargo", ".config"}:
        return None

    cur = rel.parent
    while cur != rust_root and cur.parent != cur:
        manifest = repo_root / cur / "Cargo.toml"
        if manifest.is_file():
            rel_member = cur.relative_to(rust_root)
            if _is_excluded_rust_dir(rel_member, excluded_dirs):
                return None
            try:
                text = manifest.read_text(encoding="utf-8")
            except OSError:
                return None
            return _read_package_name(text)
        cur = cur.parent
    return None


def plan_affected(
    changed_files: Iterable[str], repo_root: Path = REPO_ROOT
) -> VerificationPlan:
    """Map changed repository-relative file paths to affected verification targets."""
    normalized = tuple(sorted({_normalize_rel_path(p) for p in changed_files if p}))
    webui = False
    go_workspace = False
    go_packages: set[str] = set()
    rust_packages: set[str] = set()
    rust_fallback = False
    excluded_rust_dirs: frozenset[str] | None = None

    for rel_path in normalized:
        if rel_path.startswith("mekugi-webui/"):
            webui = True
            rel = Path(rel_path)
            is_webui_go = len(rel.parts) == 2 and (
                rel.name in GO_WORKSPACE_FILES or rel.suffix == ".go"
            )
            if is_webui_go:
                if rel.name in GO_WORKSPACE_FILES:
                    go_workspace = True
                else:
                    pkg = resolve_go_package(
                        rel_path, repo_root, root_dir="mekugi-webui"
                    )
                    if pkg is None:
                        go_workspace = True
                    else:
                        go_packages.add(pkg)
            continue

        if rel_path.startswith("mekugi-tui/"):
            if go_workspace:
                continue
            pkg = resolve_go_package(rel_path, repo_root, root_dir="mekugi-tui")
            if pkg is None:
                go_workspace = True
            else:
                go_packages.add(pkg)
            continue

        if rel_path.startswith("codex-rs/"):
            if excluded_rust_dirs is None:
                excluded_rust_dirs = _load_rust_excluded_dirs(repo_root)
            pkg = resolve_rust_package(rel_path, repo_root, excluded_rust_dirs)
            if pkg is None:
                rust_fallback = True
            else:
                rust_packages.add(pkg)
            if rel_path.startswith("codex-rs/app-server-protocol/schema/json/"):
                go_packages.add("./mekugi-tui/apps/mekugi/internal/launch")
            if rel_path == "codex-rs/app-server-protocol/src/protocol/common.rs":
                go_packages.add("./mekugi-tui/apps/mekugi/internal/launch")
            if rel_path.startswith("codex-rs/app-server-protocol/schema/typescript/"):
                webui = True

    resolved_go = ("@workspace",) if go_workspace else tuple(sorted(go_packages))
    if resolved_go and resolved_go != ("@workspace",):
        wildcards = [pkg[:-3] for pkg in resolved_go if pkg.endswith("/...")]
        if wildcards:
            resolved_go = tuple(
                pkg
                for pkg in resolved_go
                if pkg.endswith("/...")
                or not any(pkg.startswith(prefix) for prefix in wildcards)
            )
    resolved_rust = tuple(sorted(rust_packages))
    return VerificationPlan(
        changed_files=normalized,
        webui=webui,
        go_packages=resolved_go,
        rust_packages=resolved_rust,
        rust_fallback=rust_fallback,
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "base_pos",
        nargs="?",
        default=None,
        help="git base ref to diff against (default: origin/main)",
    )
    parser.add_argument(
        "--base",
        dest="base_flag",
        default=None,
        help="git base ref to diff against (overrides positional base)",
    )
    parser.add_argument(
        "--just",
        dest="just_bin",
        default="just",
        help="just executable path to invoke",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print affected verification commands without executing them",
    )
    args = parser.parse_args(argv)
    args.base = args.base_flag or args.base_pos or "origin/main"
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        changed = collect_changed_files(REPO_ROOT, args.base)
    except RuntimeError as error:
        print(f"affected: {error}", file=sys.stderr)
        return 1

    plan = plan_affected(changed, REPO_ROOT)
    commands = plan.commands(just_bin=args.just_bin)
    if not commands:
        print(
            f"No affected verification targets for {len(plan.changed_files)} changed file(s) "
            f"(base: {args.base}); skipping."
        )
        return 0

    for command in commands:
        rendered = shlex.join(command)
        if args.dry_run:
            print(f"[dry-run] {rendered}")
            continue
        print(f"==> {rendered}", flush=True)
        process = subprocess.run(command, cwd=REPO_ROOT, check=False)
        if process.returncode != 0:
            return process.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
