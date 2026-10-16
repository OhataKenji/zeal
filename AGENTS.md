# AGENTS.md

Shared ground rules for any coding agent (Claude Code, Codex, Cursor, Devin, ...) and human contributor.
Subdirectory notes: [src-tauri/AGENTS.md](src-tauri/AGENTS.md) (Rust core), [src/AGENTS.md](src/AGENTS.md) (UI).

## Read first

1. [README.md](README.md)
2. [docs/prd.md](docs/prd.md)
3. [docs/design/overview.md](docs/design/overview.md)
4. The `docs/design/mN-*.md` for the milestone you are working on
5. [docs/mock/](docs/mock/) (UI wireframe)

## Design principles

Summary only; [docs/design/overview.md](docs/design/overview.md) is the source of truth.

- Separate structure from observation; show what was not observed as unknown, never guess.
- Never write anything inside the opened Bazel workspace (bazel's own convenience symlinks excepted).
- Always call bazel as a subprocess with an argument array, never through a shell.

## Checks

Run the checks listed in the [Checks section of README.md](README.md#checks) before opening a PR.

## Pull requests

- Keep PRs small (Google small-CL): about 200-300 lines excluding tests.
- Follow [.github/pull_request_template.md](.github/pull_request_template.md); include one mermaid diagram in What.
- In Quality Assurance, do not repeat what CI already checks (build, lint, tests). PR bodies may be in Japanese.

## Design docs

- Follow [docs/templates/design_doc.md](docs/templates/design_doc.md).
- Only the owner sets Status to Approved; agents never change Status.
- If the implementation diverges from a design doc, update the doc in the same PR.

## Public repository

This repository is PUBLIC. Do not commit or push private or sensitive data.

- Raw Bazel output (BEP etc.) contains HOME, USER, hostname and client env; never commit it as is.
- Generate fixtures under `/tmp` with `--output_user_root=/tmp/...` and `--incompatible_strict_action_env`, and check them for personal data before committing.

## Environment

- Commit with `mise exec -- git commit` so the lefthook pre-commit hook can find gitleaks.
- Tauri system dependencies (Linux): see the `apt-get install` list in [environment.yaml](environment.yaml).
- `pnpm tauri dev` needs a display; in a headless environment use `pnpm dev`.
