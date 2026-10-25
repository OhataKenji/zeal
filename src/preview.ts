// Browser-only development preview. Never loaded by a production build.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { AppState, Profile, RunRecord, Snapshot, Target } from "./lib/types";
import type { AnalyticsFilter } from "./lib/analytics";

const workspace = "/demo/acme-mono";
const now = Date.now();
function target(label: string, ruleClass = "java_library"): Target {
  const [pkg, name] = label.slice(2).split(":");
  return {
    label,
    pkg,
    name,
    ruleClass,
    location: { file: `${workspace}/${pkg}/BUILD.bazel`, line: 14 },
  };
}
const parser = "//app/lib:parser";
const deps = ["parser_proto", "lexer_base", "ast_nodes", "token_stream"].map((name) =>
  target(`//app/lib:${name}`, name === "parser_proto" ? "proto_library" : "java_library"),
);
const common = [
  "diagnostics",
  "source_location",
  "utf8_table",
  "arena_pool",
  "hash_map",
  "string_intern",
  "immutable",
  "flags",
].map((name) => target(`//common:${name}`));
const users = [
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
const sourceFiles = [
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
const snapshot: Snapshot = {
  meta: { fetchedAt: now, status: "ok" },
  targets: [
    target(parser),
    ...users,
    ...deps,
    ...common,
    target("//app:grammars", "filegroup"),
    target("//app/lib:unicode_tables", "filegroup"),
  ],
  edges: [
    ...[...deps, ...common].map((dep) => ({ from: parser, to: dep.label, attr: "deps" })),
    ...[
      "@maven//:guava",
      "@maven//:slf4j_api",
      "@protobuf//:protobuf_java",
      "@maven//:jackson_core",
      "@maven//:jackson_databind",
    ].map((to) => ({ from: parser, to, attr: "deps" })),
    ...users.map((user) => ({ from: user.label, to: parser, attr: "deps" })),
    ...["hdrs", "srcs"].map((attr) => ({ from: users[0].label, to: parser, attr })),
    ...[
      "//app:grammars",
      "//app/lib:unicode_tables",
      "//app/lib:reserved_keywords.txt",
      "//app/lib:operator_prec.json",
      "@tzdata//:zoneinfo",
    ].map((to) => ({ from: parser, to, attr: "data" })),
  ],
  files: [
    ...sourceFiles.map((name) => ({ path: `app/lib/${name}`, owner: parser, role: "srcs" })),
    ...["libparser.jar", "libparser-src.jar"].map((name) => ({
      path: `app/lib/${name}`,
      owner: parser,
      role: "out",
    })),
  ],
};
function dayString(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
const records: RunRecord[] = Array.from({ length: 30 }, (_, d) => {
  const day = new Date(now);
  day.setDate(day.getDate() - d);
  return Array.from({ length: (d * 7 + 3) % 9 }, (_, n) => {
    const startedAt = day.getTime() - n * 600000;
    return {
      id: `preview-${d}-${n}`,
      workspace,
      cmd: n % 3 === 1 ? "test" : "build",
      target: n % 3 === 1 ? "//app/lib:all" : "//app:server",
      startedAt,
      finishedAt: startedAt + (n === 0 ? 9420 : 4180),
      exitCode: n === 2 ? 1 : 0,
    };
  });
}).flat();
const state: AppState = {
  workspace,
  bazel: "8.7.0",
  ingest: "idle",
  snapshot,
  lastError: null,
  running: null,
  stopping: false,
  runNote: null,
  bes: { endpoint: "grpc://127.0.0.1:4718", error: null },
};
const consoleOutput = new Map<string, string>();
const consoleInput = new Map<string, string>();
const span = (name: string, ts: number, dur: number) => ({
  name,
  ts: ts * 1e6,
  dur: dur * 1e6,
  ph: "X",
  cat: "action processing",
});
const profile: Profile = {
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

export function installPreview() {
  if (!import.meta.env.DEV || isTauri()) throw new Error("Preview requires a development browser");
  if (new URLSearchParams(location.search).get("preview") === "empty") {
    state.workspace = null;
    state.snapshot = null;
  }
  mockIPC(
    (command, args) => {
      const payload = args as Record<string, unknown> | undefined;
      switch (command) {
        case "get_state":
          return structuredClone(state);
        case "list_runs":
          return structuredClone(records);
        case "get_live_run": {
          const record = records.find((run) => run.id === payload?.id);
          if (!record) throw new Error("Preview run not found");
          return {
            record: structuredClone(record),
            log: {
              status: "ok",
              data:
                consoleOutput.get(record.id) ??
                `\u001b[32mINFO:\u001b[0m ${record.cmd} ${record.target}\n\u001b[33mWARNING:\u001b[0m Example warning in preview data\n${record.finishedAt ? "Build finished\n" : "Building…\n"}`,
            },
            bep: { status: "ok", data: { targets: [], tests: [], truncated: false } },
            build: null,
          };
        }
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
          const data = payload?.data as number[];
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
            const record = records.find((run) => run.id === id)!;
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
                truncated: false,
              },
            },
            profile: { status: "ok", data: profile },
          };
        }
        case "get_analytics": {
          const requested = payload?.filter as AnalyticsFilter;
          const from = new Date(now);
          from.setDate(from.getDate() - 29);
          const filter = {
            ...requested,
            start: requested.start || dayString(from),
            end: requested.end || dayString(new Date(now)),
          };
          const filtered = records.filter((run) => {
            const day = dayString(new Date(run.startedAt));
            const outcome =
              run.exitCode === 0 ? "ok" : run.exitCode === 8 ? "interrupted" : "failed";
            return (
              run.exitCode !== undefined &&
              day >= filter.start &&
              day <= filter.end &&
              (!filter.cmd || run.cmd === filter.cmd) &&
              (!filter.outcome || outcome === filter.outcome)
            );
          });
          const days = [];
          for (
            const d = new Date(`${filter.start}T12:00:00`);
            dayString(d) <= filter.end && days.length < 366;
            d.setDate(d.getDate() + 1)
          ) {
            const day = dayString(d);
            days.push({
              day,
              count: filtered.filter((run) => dayString(new Date(run.startedAt)) === day).length,
            });
          }
          return {
            filter,
            today: dayString(new Date(now)),
            days,
          };
        }
        case "open_workspace":
          state.workspace = workspace;
          state.snapshot = snapshot;
          return structuredClone(state);
        case "refresh":
          void emit("app-state", structuredClone(state));
          return;
        case "open_location":
          return;
        case "start_run": {
          const record: RunRecord = {
            id: `preview-${crypto.randomUUID()}`,
            workspace,
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
