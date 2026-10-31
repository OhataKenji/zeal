# Style Guide

Follow the configured formatter and linter when their rules conflict with the guides below. Otherwise, follow the relevant Google style guide.

- [Rust](https://google.github.io/styleguide/rustguide.html)
- [TypeScript](https://google.github.io/styleguide/tsguide.html)
- [HTML and CSS](https://google.github.io/styleguide/htmlcssguide.html)

## Rust tests

- Put unit tests in the same file as the code they test, inside a
  `#[cfg(test)] mod tests` module.
- Put integration tests in `src-tauri/tests/` and use the library's public API.
- Tests that directly exercise private implementation details stay with the
  implementation, even when they use subprocesses or local I/O.

This is a project convention, following the distinction in the
[Rust Book's test organization chapter](https://doc.rust-lang.org/book/ch11-03-test-organization.html).
