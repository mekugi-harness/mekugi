#!/usr/bin/env python3
"""Fingerprint the native package and native test inputs at the checked-out tree."""

import hashlib
import os
from pathlib import Path
import subprocess


def package_input(path: bytes) -> bool:
    if path.startswith(b"codex-rs/"):
        # Cargo still reads workspace manifests during a production build.
        if path.endswith(b"/Cargo.toml"):
            return True
        crate_path = path.removeprefix(b"codex-rs/")
        if crate_path.startswith((b"ext/", b"utils/")):
            crate_path = crate_path.split(b"/", 1)[1]
        # Integration suites and standalone #[cfg(test)] modules are not package
        # inputs. Retain all other Rust data, including packaged protocol schemas.
        return (
            not crate_path.startswith(b"app-server-subprocess-tests/")
            and not crate_path.partition(b"/")[2].startswith(b"tests/")
            and not path.endswith((b"/tests.rs", b"_tests.rs"))
        )
    return not path.startswith(b"scripts/codex_package/smoke_tests/") and not (
        path.startswith(b"scripts/codex_package/test_") and path.endswith(b".py")
    )


def fingerprint(runner: bytes, tree: list[bytes]) -> str:
    digest = hashlib.sha256(runner)
    for entry in tree:
        digest.update(entry + b"\0")
    return digest.hexdigest()


def tree_entries(*paths: str) -> list[bytes]:
    return subprocess.check_output(
        ["git", "ls-tree", "-rz", "HEAD", "--", *paths]
    ).split(b"\0")[:-1]


def main():
    platform = os.environ["PLATFORM"]
    profile = os.environ["NATIVE_PACKAGE_PROFILE"]
    image = [os.environ["RUNNER_LABEL"], os.environ.get("NATIVE_BUILD_IMAGE", "")]
    roots = [
        ".cargo",
        "codex-rs",
        "runtime/build.py",
        "scripts/codex_package",
        "third_party/v8",
        "LICENSE",
        "NOTICE",
        ".github/scripts/native-cargo.sh",
    ]
    if platform.startswith("linux_"):
        roots.extend((".github/ci-image", ".github/workflows/ci-image.yml"))
    # PRs and main smoke-test the archive on each runner. Release keys stay
    # exact to the runner image, or to this attempt if its version is unknown.
    if profile != "dev-small":
        image.append(
            os.environ.get("ImageVersion")
            or (f"{os.environ['GITHUB_RUN_ID']}-{os.environ['GITHUB_RUN_ATTEMPT']}")
        )
    runner = "\0".join([platform, profile, *image]).encode() + b"\0"

    tree = tree_entries(*roots)
    package_tree = [entry for entry in tree if package_input(entry.split(b"\t", 1)[1])]
    test_policy = tree_entries(
        ".github/workflows/release.yml",
        ".github/scripts/native-keys.py",
        ".github/scripts/plan-native.sh",
        ".github/scripts/reuse-pr-validation.sh",
    )
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        print(
            f"archive=mekugi_{platform}-{fingerprint(runner, package_tree)}",
            file=output,
        )
        print(
            f"tests=native-tests-{platform}-{fingerprint(runner, tree + test_policy)}",
            file=output,
        )


if __name__ == "__main__":
    main()
