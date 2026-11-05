# AGENTS.md

See [README.md](README.md) for the project overview and
[docs/development.md](docs/development.md) for development setup and checks.

## Workflow

- Before editing or reviewing code, read and follow
  [docs/styleguide.md](docs/styleguide.md), including the linked
  guide for the relevant language.
- Before creating or updating a PR description, read and follow
  [.github/pull_request_template.md](.github/pull_request_template.md),
  including instructions in HTML comments.

## Compatibility

This project is still in development. Do not preserve backward
compatibility unless explicitly requested. Implement changes as if the
new design had been in place from the beginning: remove obsolete paths
rather than adding compatibility layers or legacy fallbacks, and update
affected callers, tests, and documentation together.

## Policy

- This repository is PUBLIC. Do not commit or push private or sensitive data.
- Store temporary files outside the repository, for example in `/tmp`.
  Set browser profile directories, including Chrome's `--user-data-dir`,
  outside the repository as well.
