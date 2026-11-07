import os
from pathlib import Path
import shutil
import subprocess
import tempfile

fixture = Path(__file__).resolve().parent
bazel = os.environ.get("BAZEL") or shutil.which("bazelisk") or shutil.which("bazel")
if not bazel:
    raise SystemExit("Set BAZEL to Bazel 9.3.2 or install Bazelisk.")
env = {**os.environ, "USE_BAZEL_VERSION": "9.3.2"}
with tempfile.TemporaryDirectory(prefix="zeal-query-fixture-", dir="/tmp") as workspace:
    for name in (".bazelversion", "MODULE.bazel", "BUILD.bazel", "defs.bzl", "input.txt", "other.txt"):
        shutil.copyfile(fixture / name, Path(workspace) / name)
    version = subprocess.check_output([bazel, "--version"], cwd=workspace, env=env, text=True).strip()
    if version != "bazel 9.3.2":
        raise SystemExit(f"Expected bazel 9.3.2, got {version}")
    output = subprocess.check_output([
        bazel, "--ignore_all_rc_files", "--batch", "query",
        'let selected = "//:main" in $selected union kind("source file", deps($selected, 1))',
        "--output=proto", "--noimplicit_deps", "--keep_going",
    ], cwd=workspace, env=env)
    (fixture / "query.pb").write_bytes(output)
