"""Check configured specifications, then check known-bad transitions."""

import os
from pathlib import Path
from shutil import copyfile
from subprocess import PIPE, STDOUT, run
from tempfile import TemporaryDirectory


SPECS = Path(__file__).resolve().parent


def tlc(model):
    return [os.getenv("JAVA", "java"), "-XX:+UseParallelGC", "-cp",
            os.environ["TLA2TOOLS_JAR"], "tlc2.TLC", "-workers", "1", model]


def check_models():
    with TemporaryDirectory(prefix="zelith-tlc-") as tmp:
        configs = sorted(SPECS.glob("*.cfg"))
        modules = sorted(SPECS.glob("*.tla"))
        if not configs or {p.stem for p in configs} != {p.stem for p in modules}:
            raise SystemExit("Expected a .cfg and .tla pair for every model")
        for path in [*configs, *modules]:
            copyfile(path, Path(tmp) / path.name)
        for config in configs:
            print(f"Checking {config.stem}", flush=True)
            run(tlc(config.stem), cwd=tmp, check=True)


def check_mutation(model, old, new, status, diagnostic):
    source = (SPECS / f"{model}.tla").read_text()
    if source.count(old) != 1:
        raise SystemExit(f"{model}: mutation no longer matches exactly one transition")

    with TemporaryDirectory(prefix=f"zelith-mutant-{model}-") as tmp:
        path = Path(tmp)
        (path / f"{model}.tla").write_text(source.replace(old, new))
        copyfile(SPECS / f"{model}.cfg", path / f"{model}.cfg")
        result = run(tlc(model), cwd=tmp, stdout=PIPE, stderr=STDOUT, text=True)
        if result.returncode != status or diagnostic not in result.stdout.splitlines():
            raise SystemExit(f"{model}: expected {diagnostic!r}, got exit "
                             f"{result.returncode}\n{result.stdout}")
        print(f"{model}: rejected as expected: {diagnostic}", flush=True)


if __name__ == "__main__":
    check_models()
    check_mutation(
        "History", 'THEN /\\ phase\' = "reading" /\\ outstanding\' = outstanding - 1 + 1',
        'THEN /\\ phase\' = "idle" /\\ outstanding\' = outstanding - 1', 13,
        "Error: Action property ServeQueued is violated.",
    )
