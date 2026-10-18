# M4 run history validation

M4 records completed GUI build/test invocations through local BES and preserves their results independently of detail files. Automated checks, real Bazel uploads, retry and shutdown regressions pass. These measurements cover synthetic local workloads; they establish bounded behavior, not a performance target for arbitrary repositories.

## Revision and checks

- Measurement source revision: `83c4de88dfa3a3be6dbee3c1e9c51a1482f28870` (base `f6dd9ccdfe3e2228aed61039ed1abef3fe635971`).
- Date: 2026-10-18. Bazel 8.7.0; macOS 27.0.1; Apple M4; 32 GiB RAM. Rust 1.99.0; debug test build.
- `mise run check` passed with frontend 94 tests, type check, format, lint and build. Final Rust format, clippy and 77 tests pass at `48223c08a6ef256e5c3ea1562b43bce6d98496e0`, including four additional malformed-UTF-8 log regressions. The 12 focused profile tests also pass.
- Three manual tests pass: actual Bazel commit/ACK round trip, real-build measurements and generated near-limit profiles.
- Browser check: a saved invocation opens text log and the existing 61-thread M2 profile, phase bands and resource counters. HTML-like log text remains literal text.
- Secret scanning passed at each implementation commit.

The transport tests cover EOF before ACK, incomplete disconnect, simultaneous streams, lost/all and partial ACK replay, suffix replay, deleted-log non-repair, cross-session ID reuse, message/stream/event limits, unknown fields and nonminimal varints. SQLite-blocked tests cover close and app exit racing a started commit, including an already-reaped process. Storage tests cover parent-directory sync ordering and failure before commit, private file modes, special files, workspace membership, schema errors, orphan conflicts, truncation and immutable summaries after artifact deletion. Detail and UI tests cover corrupt gzip members, byte/event/IPC limits and stale selections. Corrupted logs use a bounded UTF-8 conversion: neither the constructed text nor its reserved capacity exceeds the output cap. This follow-up changes log decoding; the measurements below remain attributed to their recorded revision.

## Real Bazel measurements

The generated workspace contains 65 trivial genrules. `//:one` selects one; `//...` selects all 65. No user repository or data is used. Each condition has three samples. `--batch` starts a new JVM each sample. Only the first workspace sample is cold; later samples reuse action/disk caches. Baseline runs first, then slim and noslim; these small sequential samples do not establish a causal overhead estimate. No clean, shutdown or output-base override is applied to an existing workspace.

Baseline has no observation flags. Slim uses the production BES/profile flags except `--noslim_profile`. Noslim uses production flags. Process time includes client startup and upload waiting; it is separate from the stored BEP elapsed time.

| Target   | Observation | Process ms, samples 1 / 2 / 3 |
| -------- | ----------- | ----------------------------- |
| `//:one` | baseline    | 6396 / 1431 / 1043            |
| `//:one` | slim        | 1513 / 1518 / 1515            |
| `//:one` | noslim      | 1517 / 1628 / 1465            |
| `//...`  | baseline    | 1420 / 1044 / 1042            |
| `//...`  | slim        | 1619 / 1558 / 1578            |
| `//...`  | noslim      | 1556 / 1576 / 1781            |

The following values are medians of three samples. Detail time includes protected database/file opens and parsing. IPC bytes are the complete serialized detail, including log and summary. Every observed build produces one committed invocation and a matching profile.

| Target   | Mode   | BES bytes | Log bytes | Profile gzip / expanded bytes | Detail ms | IPC bytes |
| -------- | ------ | --------: | --------: | ----------------------------: | --------: | --------: |
| `//:one` | slim   |    47,067 |       640 |              13,384 / 116,732 |    79.734 |   107,786 |
| `//:one` | noslim |    47,079 |       500 |              13,421 / 116,372 |    66.153 |   107,490 |
| `//...`  | slim   |   121,815 |       589 |              16,702 / 156,629 |    85.869 |   143,991 |
| `//...`  | noslim |   121,781 |       449 |              16,689 / 156,581 |    78.342 |   143,787 |

The harness reports a peak self RSS of 19,283,968 bytes. External `/usr/bin/time -l` reports 436,518,912 bytes for the run involving Bazel child processes. Neither value is the GUI application's standalone memory use. Payload bytes, expanded bytes and RSS measure different things.

## Large inputs

The five profile cases run once each, sequentially in one dedicated measurement process. Generated padding and events contain no project data.

| Case                  | Expanded bytes | Gzip bytes |  Events | Result IPC bytes |  Parse ms | Result     |
| --------------------- | -------------: | ---------: | ------: | ---------------: | --------: | ---------- |
| Near 64 MiB           |     67,108,794 |    305,686 |       1 |               82 | 1,624.525 | ok         |
| Over 64 MiB           |     67,108,922 |    305,688 |       1 |                0 | 1,641.001 | incomplete |
| At event limit        |      6,250,033 |     42,251 | 250,000 |        6,250,088 |   659.191 | ok         |
| Over event limit      |      6,250,058 |     42,251 | 250,001 |                0 |   700.146 | incomplete |
| Event over IPC budget |     33,562,694 |    152,981 |       2 |                0 | 1,632.605 | incomplete |

The five-case process peaks at 266,911,744 RSS bytes; the OS separately reports 190,235,368 peak footprint bytes. This includes retained JSON values and allocator memory. Buffering after the expanded-byte limiter reduced the near-64-MiB case from an exploratory 14,444 ms to 1,625 ms without relaxing any limit.

A separate local transport test sends ten unknown 7-MiB BEP bodies. It rejects the stream after the 64-MiB budget is crossed, commits no record and leaves no saved log. The test process includes both gRPC endpoints; it takes 0.31 s and peaks at 47,595,520 RSS bytes. Oversized messages and 250,001-envelope streams have separate passing transport tests. Limit rejection stops processing after at most the next bounded message/event; unknown inputs count too.

The detail worker admits one request and rejects concurrent requests as busy. There is no waiting queue or mid-parse cancellation; selection generations discard old results. These samples measure synchronous detail work, not webview scheduling or end-to-end repaint latency.

## Reproduction and operation

```sh
mise run check
cargo test --manifest-path src-tauri/Cargo.toml --lib bes::tests::real_bazel_round_trip -- --ignored --exact --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --lib acceptance::measure_real_bazel -- --ignored --exact --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --lib profile::tests::measure_profile_limits -- --ignored --exact --nocapture
```

For RSS, first build the test executable with `cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run`, then run that printed executable under `/usr/bin/time -l` with one selector above. This excludes compiler memory. Each measured workload must finish before the next starts.

History starts with a new schema; legacy run directories are not migrated. Database failures prevent new build/test processes, while the held process can still be stopped. There is no repair or automatic orphan adoption. Manual cleanup requires stopping every Zeal instance and remaining process. Remove only detail files and temporary directories. Manual database initialization requires backing up the database and sidecars after the same stop check and loses history for every workspace. See [M4](../design/m4-run-history.md) for the storage contract and failure classes.
