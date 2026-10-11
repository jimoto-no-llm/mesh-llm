from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "mesh" / "scripts" / "build-ui.sh"
HOST_SCRIPT = ROOT / "mesh" / "scripts" / "build-host.sh"
PACKAGE_SCRIPT = ROOT / "mesh" / "scripts" / "package-sdk-console-assets.sh"
ENSURE_SCCACHE_LIB = ROOT / "scripts" / "lib" / "ensure-sccache.sh"
UI_DIR = ROOT / "mesh" / "crates" / "mesh-llm-ui"
WORKFLOWS = ROOT / ".github" / "workflows"


def _parse_partial_version(text: str) -> tuple[int, int, int]:
    parts = (text.split(".") + ["0", "0"])[:3]
    return tuple(int(part) for part in parts)


def _version_satisfies_range(version: str, range_spec: str) -> bool:
    """Check a concrete x.y.z version against a small, space-separated-AND
    subset of node-semver ranges: `>=`, `<=`, `>`, `<`, and bare-equals
    comparators only.

    `engines.pnpm` in this repo has never needed `^`, `~`, or `||`; if it
    grows one, extend this rather than reaching for a dependency.
    """
    version_tuple = _parse_partial_version(version)
    comparator = re.compile(r"^(>=|<=|>|<|=)?(\d+(?:\.\d+){0,2})$")
    for token in range_spec.split():
        match = comparator.match(token)
        if match is None:
            raise ValueError(f"unsupported engines.pnpm range token: {token!r}")
        operator, bound = match.group(1) or "=", match.group(2)
        bound_tuple = _parse_partial_version(bound)
        satisfied = {
            ">=": version_tuple >= bound_tuple,
            "<=": version_tuple <= bound_tuple,
            ">": version_tuple > bound_tuple,
            "<": version_tuple < bound_tuple,
            "=": version_tuple == bound_tuple,
        }[operator]
        if not satisfied:
            return False
    return True


class SemverRangeTests(unittest.TestCase):
    """Direct coverage for the range check the engines.pnpm test relies on."""

    def test_satisfies_a_lower_bound(self) -> None:
        self.assertTrue(_version_satisfies_range("10.30.3", ">=10"))

    def test_rejects_a_pin_below_an_exclusive_upper_bound_stated_as_a_lower_bound(
        self,
    ) -> None:
        self.assertFalse(_version_satisfies_range("10.30.3", "<10"))

    def test_rejects_a_pin_outside_a_narrow_conjunctive_range(self) -> None:
        self.assertFalse(_version_satisfies_range("10.30.3", ">=10 <10.30.0"))

    def test_satisfies_a_wide_conjunctive_range(self) -> None:
        self.assertTrue(_version_satisfies_range("10.30.3", ">=10 <11"))


class BuildUiScriptTests(unittest.TestCase):
    def test_rust_build_script_does_not_watch_a_missing_dist_directory(self) -> None:
        build_script = (UI_DIR / "build.rs").read_text(encoding="utf-8")

        self.assertNotIn('cargo:rerun-if-changed=dist', build_script)
        self.assertIn("Cargo's default package change detection", build_script)

    def test_ui_pnpm_workspace_names_root_package(self) -> None:
        workspace = ROOT / "mesh" / "crates" / "mesh-llm-ui" / "pnpm-workspace.yaml"

        contents = workspace.read_text(encoding="utf-8")

        self.assertIn("packages:", contents)
        self.assertRegex(contents, r"(?m)^\s*-\s*['\"]?\.[\"']?\s*$")

    def ci_pnpm_majors(self) -> set[str]:
        """Every pnpm major `pnpm/action-setup` is asked to install in CI."""
        majors: set[str] = set()
        workflows = [*WORKFLOWS.glob("*.yml"), *WORKFLOWS.glob("*.yaml")]
        for workflow in sorted(workflows):
            for block in re.finditer(
                r"uses:\s*pnpm/action-setup@[^\n]*\n"
                r"(?P<rest>(?:[ \t]+(?![ \t]*-)[^\n]*\n)*)",
                workflow.read_text(encoding="utf-8"),
            ):
                version = re.search(
                    r"(?m)^[ \t]*version:\s*[\"']?(\d+)", block.group("rest")
                )
                if version:
                    majors.add(version.group(1))
        return majors

    def test_ui_package_declares_the_pnpm_major_that_ci_installs(self) -> None:
        """The lockfile only installs under pnpm 10+, so the repo must say so.

        `overrides` and `allowBuilds` live in `pnpm-workspace.yaml`, which is a
        pnpm 10 layout. pnpm 9 does not read them, so it computes a different
        overrides config than the lockfile records and stops with
        `ERR_PNPM_LOCKFILE_CONFIG_MISMATCH` — a message that says nothing about
        pnpm versions. Declaring the requirement turns that into
        `ERR_PNPM_UNSUPPORTED_ENGINE`, which names the actual problem.
        """
        manifest = json.loads((UI_DIR / "package.json").read_text(encoding="utf-8"))

        required = manifest.get("engines", {}).get("pnpm")
        self.assertIsNotNone(required, "package.json must declare engines.pnpm")
        declared_major = re.search(r"(\d+)", required)
        self.assertIsNotNone(declared_major)
        assert declared_major is not None

        pinned = manifest.get("packageManager", "")
        self.assertRegex(
            pinned,
            r"^pnpm@\d+\.\d+\.\d+$",
            "packageManager must pin an exact pnpm version for corepack",
        )

        ci_majors = self.ci_pnpm_majors()
        self.assertTrue(ci_majors, "no pnpm/action-setup version found to compare against")
        self.assertEqual(
            ci_majors,
            {declared_major.group(1)},
            "engines.pnpm and CI's pnpm/action-setup version have drifted apart",
        )
        pinned_version = pinned.removeprefix("pnpm@")
        self.assertTrue(
            _version_satisfies_range(pinned_version, required),
            f"packageManager {pinned} does not satisfy the declared engines.pnpm range {required!r}",
        )

    def test_ui_npmrc_enforces_the_declared_engine(self) -> None:
        """`engines` is advisory to pnpm unless engine-strict is turned on.

        Without this the field is documentation only and a pnpm 9 operator
        still gets the unrelated lockfile-mismatch error.
        """
        npmrc = UI_DIR / ".npmrc"
        self.assertTrue(npmrc.exists(), "mesh/crates/mesh-llm-ui/.npmrc is missing")
        self.assertRegex(
            npmrc.read_text(encoding="utf-8"),
            r"(?m)^engine-strict\s*=\s*true\s*$",
        )

    def test_ui_toolchain_checked_before_pnpm_install(self) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('source "$SCRIPT_DIR/lib/ensure-ui-tools.sh"', script)
        check = script.index("ensure_ui_tools")
        install = script.index("pnpm install --frozen-lockfile")
        self.assertLess(check, install)

    def test_missing_ui_toolchain_fails_with_guidance(self) -> None:
        helper = ROOT / "scripts" / "lib" / "ensure-ui-tools.sh"
        with tempfile.TemporaryDirectory() as directory:
            missing = subprocess.run(
                ["/bin/bash", "-c", f'source "{helper}" && ensure_ui_tools'],
                env={"PATH": directory},
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn("pnpm", missing.stderr)
            self.assertIn("corepack", missing.stderr)
            self.assertIn("MESH_LLM_SKIP_UI=1", missing.stderr)
            self.assertIn("just build", missing.stderr)
            self.assertIn("just mesh", missing.stderr)
            self.assertIn("build-host.sh", missing.stderr)
            self.assertIn("--skip-build", missing.stderr)
            for name in ("node", "pnpm"):
                stub = Path(directory) / name
                stub.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
                stub.chmod(0o755)
            present = subprocess.run(
                ["/bin/bash", "-c", f'source "{helper}" && ensure_ui_tools'],
                env={"PATH": directory},
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(present.returncode, 0, present.stderr)

    def test_skip_env_is_honored_only_through_the_host_wrapper(self) -> None:
        """The guided skip works via `just build` / `just mesh` only.

        `build-host.sh` reads `MESH_LLM_SKIP_UI`; direct `build-ui.sh` calls
        ignore it and `package-sdk-console-assets.sh` uses `--skip-build`, so
        the guidance must name the supported entry points.
        """
        host = HOST_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("MESH_LLM_SKIP_UI", host)
        ui = SCRIPT.read_text(encoding="utf-8")
        self.assertNotIn("MESH_LLM_SKIP_UI", ui)
        package = PACKAGE_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("--skip-build", package)
        self.assertNotIn("MESH_LLM_SKIP_UI", package)

    def test_guided_skip_env_skips_the_host_ui_step_without_toolchain(self) -> None:
        """Running the guided `export MESH_LLM_SKIP_UI=1` via build-host.sh
        skips the UI without needing node or pnpm."""
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            scripts_dir = tmp / "scripts"
            mesh_scripts_dir = tmp / "mesh" / "scripts"
            bin_dir = tmp / "bin"
            mesh_scripts_dir.mkdir(parents=True)
            scripts_dir.mkdir(exist_ok=True)
            bin_dir.mkdir(exist_ok=True)
            lib_dir = scripts_dir / "lib"
            lib_dir.mkdir(exist_ok=True)
            shutil.copy(ENSURE_SCCACHE_LIB, lib_dir / "ensure-sccache.sh")
            for name in ("macos-deployment-target.sh", "macos-deployment-target.txt"):
                shutil.copy(ROOT / "scripts" / "lib" / name, lib_dir / name)
            copied_host = mesh_scripts_dir / "build-host.sh"
            shutil.copy(HOST_SCRIPT, copied_host)
            copied_host.chmod(copied_host.stat().st_mode | stat.S_IXUSR)
            marker = tmp / "ui-called"
            self.write_executable(
                scripts_dir / "build-ui.sh",
                textwrap.dedent(
                    f"""\
                    #!/usr/bin/env bash
                    set -euo pipefail
                    touch "{marker}"
                    echo "stub build-ui should have been skipped" >&2
                    exit 2
                    """
                ),
            )
            self.write_executable(bin_dir / "sccache", "#!/bin/sh\nexit 0\n")
            self.write_executable(bin_dir / "cargo", "#!/usr/bin/env bash\nset -euo pipefail\nexit 0\n")
            env = os.environ.copy()
            env.update(
                {
                    "MESH_LLM_SKIP_UI": "1",
                    "MESH_LLM_BUILD_VERSION": "0.0.0-test",
                    "PATH": f"{bin_dir}{os.pathsep}{env['PATH']}",
                }
            )
            result = subprocess.run(
                [str(copied_host), "--profile", "debug"],
                cwd=tmp,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Skipping mesh-llm UI build because MESH_LLM_SKIP_UI=1.", result.stdout)
            self.assertFalse(marker.exists(), "build-ui.sh must not run when the guided skip is set")

    def test_direct_ui_build_ignores_the_guided_skip_env(self) -> None:
        """Direct `build-ui.sh` with `MESH_LLM_SKIP_UI=1` still demands the toolchain."""
        with tempfile.TemporaryDirectory() as tmpdir:
            ui_dir = Path(tmpdir) / "ui"
            ui_dir.mkdir()
            for relative in (
                "package.json",
                "pnpm-lock.yaml",
                "vite.config.ts",
                "tsconfig.json",
                "tsconfig.app.json",
                "tsconfig.node.json",
                "biome.json",
                "index.html",
            ):
                (ui_dir / relative).write_text("{}\n", encoding="utf-8")
            (ui_dir / "src").mkdir()
            (ui_dir / "public").mkdir()
            env = os.environ.copy()
            env["MESH_LLM_SKIP_UI"] = "1"
            # Isolated PATH with base tools only: the absence of pnpm must
            # hold inside the test, not depend on the parent environment.
            isolated_bin = Path(tmpdir) / "bin"
            isolated_bin.mkdir()
            for tool in (
                "cat", "chmod", "cut", "dirname", "find", "grep",
                "head", "mkdir", "pwd", "rm", "sh", "sort", "tr",
                "uname",
            ):
                resolved = shutil.which(tool)
                if resolved is not None:
                    (isolated_bin / tool).symlink_to(resolved)
            env["PATH"] = str(isolated_bin)
            self.assertIsNone(
                shutil.which("pnpm", path=env["PATH"]),
                "direct UI test requires pnpm to be absent from PATH",
            )
            result = subprocess.run(
                ["/bin/bash", str(SCRIPT), str(ui_dir)],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("MESH_LLM_SKIP_UI=1", result.stderr)
            self.assertIn("just build", result.stderr)

    def test_mixed_case_release_profile_uses_release_ui_env(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ui_dir = Path(tmp) / "ui"
            self._write_up_to_date_ui_fixture(ui_dir, profile="release", debug_ui="false")

            env = os.environ.copy()
            env["MESH_LLM_BUILD_PROFILE"] = "ReLeAsE"
            for key in ("VITE_BASE_PATH", "VITE_ROUTER_BASE_PATH", "VITE_STORAGE_NAMESPACE"):
                env.pop(key, None)

            result = subprocess.run(
                ["bash", str(SCRIPT), str(ui_dir)],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("profile: release", result.stdout)
        self.assertIn("debug UI: false", result.stdout)

    def write_executable(self, path: Path, content: str) -> None:
        path.write_text(textwrap.dedent(content).lstrip(), encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def _write_up_to_date_ui_fixture(self, ui_dir: Path, *, profile: str, debug_ui: str) -> None:
        ui_dir.mkdir()
        for relative in (
            "package.json",
            "pnpm-lock.yaml",
            "vite.config.ts",
            "tsconfig.json",
            "tsconfig.app.json",
            "tsconfig.node.json",
            "biome.json",
            "index.html",
        ):
            (ui_dir / relative).write_text("{}\n", encoding="utf-8")
        (ui_dir / "src").mkdir()
        (ui_dir / "public").mkdir()

        dist_dir = ui_dir / "dist"
        dist_dir.mkdir()
        (dist_dir / "asset.js").write_text("// built\n", encoding="utf-8")
        (dist_dir / ".mesh-llm-ui-build-env").write_text(
            f"MESH_LLM_BUILD_PROFILE={profile}\n"
            f"VITE_MESH_LLM_DEBUG_UI={debug_ui}\n"
            "VITE_BASE_PATH=\n"
            "VITE_ROUTER_BASE_PATH=\n"
            "VITE_STORAGE_NAMESPACE=\n",
            encoding="utf-8",
        )


if __name__ == "__main__":
    unittest.main()
