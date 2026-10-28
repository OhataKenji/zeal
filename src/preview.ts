// Browser-only development preview. Never loaded by a production build.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { AppState, Profile, RunRecord, Snapshot, Target } from "./lib/types";
import type { AnalyticsFilter } from "./lib/analytics";

const WORKSPACE = "/demo/acme-mono";
const NOW = Date.now();
function target(label: string, ruleClass = "java_library"): Target {
  const [pkg, name] = label.slice(2).split(":");
  return {
    label,
    pkg,
    name,
    ruleClass,
    location: { file: `${WORKSPACE}/${pkg}/BUILD.bazel`, line: 14 },
  };
}
const PARSER = "//app/lib:parser";
const DEPS = ["parser_proto", "lexer_base", "ast_nodes", "token_stream"].map((name) =>
  target(`//app/lib:${name}`, name === "parser_proto" ? "proto_library" : "java_library"),
);
const COMMON = [
  "diagnostics",
  "source_location",
  "utf8_table",
  "arena_pool",
  "hash_map",
  "string_intern",
  "immutable",
  "flags",
].map((name) => target(`//common:${name}`));
const USERS = [
  target("//app/lib:parser_test", "java_test"),
  target("//app/lib:parser_fuzz_test", "java_test"),
  target("//app/lib:parser_benchmark", "java_binary"),
  target("//app/lib:ast_printer", "java_binary"),
  target("//app:server", "java_binary"),
  target("//app/cli:zeal_cli", "java_binary"),
  target("//app/worker:indexer"),
  target("//tools:docgen", "java_binary"),
  target("//app/lsp:lang_server"),
  target("//app/lsp:completion_test", "java_test"),
  target("//app/lint:rule_engine"),
];
const SOURCE_FILES = [
  "Parser.java",
  "Token.java",
  "Lexer.java",
  "AstNode.java",
  "AstVisitor.java",
  "SyntaxError.java",
  "Symbol.java",
  "ScopeTable.java",
  "TypeResolver.java",
  "ExprNode.java",
  "StmtNode.java",
  "DeclNode.java",
  "SpanTracker.java",
  "GrammarRule.java",
  "LookaheadBuf.java",
  "CommentScanner.java",
  "LiteralParser.java",
  "ParserState.java",
  "TokenKind.java",
  "NodeKind.java",
  "SourceSpan.java",
  "Diagnostics.java",
];
const SNAPSHOT: Snapshot = {
  meta: { fetchedAt: NOW, status: "ok" },
  targets: [
    target(PARSER),
    ...USERS,
    ...DEPS,
    ...COMMON,
    target("//app:grammars", "filegroup"),
    target("//app/lib:unicode_tables", "filegroup"),
  ],
  edges: [
    ...[...DEPS, ...COMMON].map((dep) => ({ from: PARSER, to: dep.label, attr: "deps" })),
    ...[
      "@maven//:guava",
      "@maven//:slf4j_api",
      "@protobuf//:protobuf_java",
      "@maven//:jackson_core",
      "@maven//:jackson_databind",
    ].map((to) => ({ from: PARSER, to, attr: "deps" })),
    ...USERS.map((user) => ({ from: user.label, to: PARSER, attr: "deps" })),
    ...["hdrs", "srcs"].map((attr) => ({ from: USERS[0].label, to: PARSER, attr })),
    ...[
      "//app:grammars",
      "//app/lib:unicode_tables",
      "//app/lib:reserved_keywords.txt",
      "//app/lib:operator_prec.json",
      "@tzdata//:zoneinfo",
    ].map((to) => ({ from: PARSER, to, attr: "data" })),
  ],
  files: [
    ...SOURCE_FILES.map((name) => ({ path: `app/lib/${name}`, owner: PARSER, role: "srcs" })),
    ...["libparser.jar", "libparser-src.jar"].map((name) => ({
      path: `app/lib/${name}`,
      owner: PARSER,
      role: "out",
    })),
  ],
};
function dayString(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
const PREVIEW_TARGETS = [
  "//app:server",
  "//app/lib:all",
  "//app/cli:zeal_cli",
  "//app/lsp:completion_test",
  "//...",
];
const dayIndices = Array.from({ length: 30 }).keys();
const records: RunRecord[] = Array.from(dayIndices, (dayIndex) => {
  const dayDate = new Date(NOW);
  dayDate.setDate(dayDate.getDate() - dayIndex);
  const runCount = (dayIndex * 7 + 3) % 9;
  const runIndices = Array.from({ length: runCount }).keys();
  return Array.from(runIndices, (runIndex) => {
    const startedAt = dayDate.getTime() - runIndex * 600000;
    return {
      id: `preview-${dayIndex}-${runIndex}`,
      workspace: WORKSPACE,
      cmd: runIndex % 3 === 1 ? "test" : "build",
      external: runIndex < 2,
      target: PREVIEW_TARGETS[(dayIndex + runIndex) % PREVIEW_TARGETS.length],
      startedAt,
      // This record has an exit code but no finish time: counted, never timed.
      ...(runIndex === 4
        ? {}
        : { finishedAt: startedAt + 4180 + runIndex * 1200 + (dayIndex % 5) * 900 }),
      exitCode: runIndex === 2 ? 1 : runIndex === 5 ? 8 : 0,
    };
  });
}).flat();
const state: AppState = {
  workspace: WORKSPACE,
  bazel: "8.7.0",
  ingest: "idle",
  snapshot: SNAPSHOT,
  lastError: null,
  running: null,
  stopping: false,
  runNote: null,
};
const consoleOutput = new Map<string, string>();
const consoleInput = new Map<string, string>();
function span(name: string, ts: number, dur: number) {
  return {
    name,
    ts: ts * 1e6,
    dur: dur * 1e6,
    ph: "X",
    cat: "action processing",
  };
}
const PROFILE: Profile = {
  phases: [
    { name: "Init", startUs: 0, endUs: 300000 },
    { name: "Load", startUs: 300000, endUs: 1400000 },
    { name: "Analysis", startUs: 1400000, endUs: 3500000 },
    { name: "Execution", startUs: 2500000, endUs: 9200000 },
    { name: "End", startUs: 9200000, endUs: 9420000 },
  ],
  lanes: [
    {
      tid: 0,
      name: "critical-path",
      events: [
        span("Proto", 0.3, 1.4),
        span("Turbine :parser", 1.85, 2.4),
        span("Javac //app:server", 4.58, 2.34),
        span("SingleJar", 7.05, 1.6),
      ],
    },
    {
      tid: 32,
      name: "skyframe-evaluator-0",
      events: [
        span("Package loading", 0, 1.1),
        span("Analysis", 1.2, 1.5),
        span("Nested analysis", 1.4, 1),
        span("Resolve deps", 2.8, 1.7),
        span("Javac", 4.6, 2.5),
        span("Javac :parser", 4.8, 1.8),
        span("Link", 7.2, 2),
      ],
    },
    {
      tid: 33,
      name: "skyframe-evaluator-1",
      events: [
        span("Load", 0.2, 1.6),
        span("Analysis", 1.95, 2),
        span("Resolve", 2.1, 1.2),
        span("Proto", 4.05, 1.4),
        span("Javac", 5.55, 2.3),
        span("Nested compile", 5.75, 1.6),
        span("Finish", 8, 1.2),
      ],
    },
    {
      tid: 41,
      name: "javac-worker-0",
      events: [span("Javac :parser", 1.85, 2.4), span("Javac :server", 4.58, 3.2)],
    },
    { tid: 54, name: "grpc-nio-worker-0", events: [span("Remote cache", 0.2, 8.6)] },
    {
      tid: 60,
      name: "local action count",
      events: [0, 1, 5, 6, 7, 8, 7.8, 7.5, 6, 3, 0].map((value, i) => ({
        name: "local action count",
        ts: i * 942000,
        ph: "C",
        args: { actions: value },
      })),
    },
  ],
};

/** Installs mock core IPC for the development-only browser preview. */
export function installPreview() {
  if (!import.meta.env.DEV || isTauri()) throw new Error("Preview requires a development browser");
  if (new URLSearchParams(location.search).get("preview") === "empty") {
    state.workspace = null;
    state.snapshot = null;
  }
  mockIPC(
    (command, args) => {
      // Tauri's mock callback provides command arguments as a key-value object.
      const payload = args as Record<string, unknown> | undefined;
      switch (command) {
        case "get_state":
          return structuredClone(state);
        case "list_runs":
          return structuredClone(records);
        case "read_console": {
          const id = String(payload?.id);
          const bytes = new TextEncoder().encode(consoleOutput.get(id) ?? "");
          const offset = Number(payload?.offset ?? 0);
          const data = Array.from(bytes.slice(offset, offset + 64 * 1024));
          return {
            data,
            offset: offset + data.length,
            done: state.running !== id && offset + data.length === bytes.length,
          };
        }
        case "resize_console":
          return;
        case "write_console": {
          const id = String(payload?.id);
          const data = payload?.data;
          if (!Array.isArray(data)) {
            throw new Error("Preview console input must be a byte array.");
          }
          if (state.running !== id) throw new Error("Preview process has ended");
          const text = new TextDecoder().decode(Uint8Array.from(data));
          let output = consoleOutput.get(id) ?? "";
          let input = consoleInput.get(id) ?? "";
          let finish: number | null = null;
          for (const ch of text) {
            if (ch === "\x03" || ch === "\x04") {
              output += "^" + (ch === "\x03" ? "C" : "D") + "\r\n";
              finish = 130;
              break;
            }
            if (ch === "\r" || ch === "\n") {
              output += `\r\n${Number(input) * 2}\r\n`;
              finish = 0;
              break;
            }
            if (ch === "\x7f") {
              input = input.slice(0, -1);
              output += "\b \b";
            } else {
              input += ch;
              output += ch;
            }
          }
          consoleInput.set(id, input);
          consoleOutput.set(id, output);
          if (finish !== null) {
            const record = records.find((run) => run.id === id);
            if (!record) {
              throw new Error("Preview run was not found.");
            }
            record.finishedAt = Date.now();
            record.exitCode = finish;
            state.running = null;
            void emit("app-state", structuredClone(state));
          }
          return data.length;
        }
        case "get_run": {
          const record = records.find((run) => run.id === payload?.id);
          if (!record) throw new Error("Preview run not found");
          return {
            record,
            log: {
              status: "ok",
              data:
                consoleOutput.get(record.id) ??
                `INFO: ${record.cmd} ${record.target}\n${record.exitCode === 0 ? "INFO: Build completed successfully" : `ERROR: Run ended with exit ${record.exitCode}`}\n`,
            },
            bep: {
              status: "ok",
              data: {
                targets: [{ label: record.target, ok: record.exitCode === 0 }],
                tests: [],
                ...(record.id === records[0]?.id
                  ? {
                      options: {
                        startupOptions: ["--output_user_root=/tmp/zeal-preview-root", "--batch"],
                        explicitStartupOptions: ["--output_user_root=/tmp/zeal-preview-root"],
                        cmdLine: [
                          "--color=yes",
                          "--jobs=8",
                          "--define=message=hello world",
                          "--keep_going",
                          "--test_output=errors",
                          "--copt=-DNAME=preview value",
                          "--remote_cache=grpc://cache.example.invalid",
                        ],
                        explicitCmdLine: [
                          "--define=message=hello world",
                          "--jobs=8",
                          "--keep_going",
                          "--test_output=errors",
                          "--copt=-DNAME=preview value",
                        ],
                      },
                    }
                  : {}),
                truncated: false,
              },
            },
          };
        }
        case "get_run_profile": {
          if (!records.some((run) => run.id === payload?.id)) {
            throw new Error("Preview run not found");
          }
          return { status: "ok", data: PROFILE };
        }
        case "get_analytics": {
          // The app sends this command with the AnalyticsFilter shape.
          const requested = (payload?.filter ?? {}) as AnalyticsFilter;
          const from = new Date(NOW);
          from.setDate(from.getDate() - 29);
          const filter = {
            ...requested,
            start: requested.start ?? dayString(from),
            end: requested.end ?? dayString(new Date(NOW)),
          };
          // Mirror the backend contract: reject, never silently truncate.
          // Pure proleptic-Gregorian checks — local-time Date parsing would
          // reject days a skipped-timezone calendar still counts.
          const utcDay = (s: string): number | null => {
            const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
            if (!m) return null;
            const d = new Date(0);
            const year = Number(m[1]);
            const month = Number(m[2]);
            const day = Number(m[3]);
            d.setUTCFullYear(year, month - 1, day);
            return d.getUTCFullYear() === year &&
              d.getUTCMonth() === month - 1 &&
              d.getUTCDate() === day
              ? d.getTime()
              : null;
          };
          const first = utcDay(filter.start);
          const last = utcDay(filter.end);
          if (first === null || last === null) {
            throw new Error("invalid analytics filter");
          }
          const spanDays = (last - first) / 86400000 + 1;
          if (
            !(spanDays >= 1 && spanDays <= 366) ||
            (filter.cmd != null && !["build", "test"].includes(filter.cmd)) ||
            (filter.outcome != null && !["ok", "failed", "interrupted"].includes(filter.outcome)) ||
            (filter.target != null && new TextEncoder().encode(filter.target).length > 64 * 1024)
          ) {
            throw new Error("invalid analytics filter");
          }
          const pct = (sorted: number[], p: number) =>
            sorted.length ? sorted[Math.ceil((sorted.length * p) / 100) - 1] : 0;
          const inPeriod = records.filter((run) => {
            const day = dayString(new Date(run.startedAt));
            const outcome =
              run.exitCode === 0 ? "ok" : run.exitCode === 8 ? "interrupted" : "failed";
            return (
              run.exitCode !== undefined &&
              ["build", "test"].includes(run.cmd) &&
              day >= filter.start &&
              day <= filter.end &&
              (filter.cmd == null || run.cmd === filter.cmd) &&
              (filter.outcome == null || outcome === filter.outcome)
            );
          });
          const days = new Map<
            string,
            {
              count: number;
              timed: number;
              ok: number;
              failed: number;
              interrupted: number;
              durations: number[];
            }
          >();
          const utcString = (d: Date) =>
            `${String(d.getUTCFullYear()).padStart(4, "0")}-${String(d.getUTCMonth() + 1).padStart(2, "0")}-${String(d.getUTCDate()).padStart(2, "0")}`;
          for (
            const d = new Date(first);
            utcString(d) <= filter.end;
            d.setUTCDate(d.getUTCDate() + 1)
          ) {
            days.set(utcString(d), {
              count: 0,
              timed: 0,
              ok: 0,
              failed: 0,
              interrupted: 0,
              durations: [],
            });
          }
          const byTarget = new Map<string, { runs: number; durations: number[] }>();
          for (const run of inPeriod) {
            const duration =
              run.finishedAt === undefined ? null : Math.max(0, run.finishedAt - run.startedAt);
            const agg = byTarget.get(run.target) ?? { runs: 0, durations: [] };
            agg.runs += 1;
            if (duration !== null) agg.durations.push(duration);
            byTarget.set(run.target, agg);
            if (filter.target != null && run.target !== filter.target) {
              continue;
            }
            const slot = days.get(dayString(new Date(run.startedAt)));
            if (!slot) {
              throw new Error("Preview analytics day was not initialized.");
            }
            slot.count += 1;
            if (run.exitCode === 0) {
              slot.ok += 1;
            } else if (run.exitCode === 8) {
              slot.interrupted += 1;
            } else {
              slot.failed += 1;
            }
            if (duration !== null) {
              slot.timed += 1;
              slot.durations.push(duration);
            }
          }
          return {
            filter,
            today: dayString(new Date(NOW)),
            days: [...days].map(([day, slot]) => {
              const sorted = [...slot.durations].sort((a, b) => a - b);
              return {
                day,
                count: slot.count,
                timed: slot.timed,
                ok: slot.ok,
                failed: slot.failed,
                interrupted: slot.interrupted,
                totalMs: sorted.reduce((a, b) => a + b, 0),
                p50Ms: pct(sorted, 50),
                p95Ms: pct(sorted, 95),
              };
            }),
            targets: [...byTarget]
              .map(([target, agg]) => {
                const sorted = agg.durations.sort((a, b) => a - b);
                return {
                  target,
                  runs: agg.runs,
                  totalMs: sorted.reduce((a, b) => a + b, 0),
                  p50Ms: pct(sorted, 50),
                  p95Ms: pct(sorted, 95),
                };
              })
              .sort(
                (a, b) =>
                  b.totalMs - a.totalMs || (a.target < b.target ? -1 : a.target > b.target ? 1 : 0),
              )
              .slice(0, 10),
          };
        }
        case "open_workspace":
          state.workspace = WORKSPACE;
          state.snapshot = SNAPSHOT;
          return structuredClone(state);
        case "refresh":
          void emit("app-state", structuredClone(state));
          return;
        case "open_location":
          return;
        case "start_run": {
          const record: RunRecord = {
            id: `preview-${crypto.randomUUID()}`,
            workspace: WORKSPACE,
            cmd: String(payload?.cmd),
            target: String(payload?.target),
            startedAt: Date.now(),
          };
          records.unshift(record);
          state.running = record.id;
          consoleOutput.set(
            record.id,
            `\x1b[32mINFO:\x1b[0m bazel ${record.cmd} ${record.target}\r\nPreview Console — sample data\r\n${record.cmd === "run" ? "Number: " : "Build completed.\r\n"}`,
          );
          if (record.cmd !== "run") {
            record.finishedAt = Date.now();
            record.exitCode = 0;
            state.running = null;
          }
          void emit("app-state", structuredClone(state));
          return structuredClone(record);
        }
        case "stop_run": {
          const record = records.find((run) => run.id === state.running);
          if (record) {
            record.finishedAt = Date.now();
            record.exitCode = 8;
          }
          state.running = null;
          void emit("app-state", structuredClone(state));
          return;
        }
        default:
          throw new Error(`Unsupported preview command: ${command}`);
      }
    },
    { shouldMockEvents: true },
  );
}
