// Browser-only development preview. Never loaded by a production build.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type {
  AppState,
  Edge,
  FileEntry,
  InvocationRecord,
  Profile,
  Target,
  TargetDetail,
  TargetIndex,
} from "./lib/types";
import type { AnalyticsFilter } from "./lib/analytics";
import { previewAnalytics } from "./preview/analytics.ts";
import { interpretConsoleInput } from "./preview/console.ts";
import type { CommandName } from "./lib/commands.ts";
import { bazelBusyReason } from "./lib/session.ts";

const WORKSPACE = "/demo/cool-mono";
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
const FULL_DATA: TargetIndex & { edges: Edge[]; files: FileEntry[] } = {
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
    { path: "app/server", owner: "//app:server", role: "out" },
    { path: "app/server_deploy.jar", owner: "//app:server", role: "out" },
  ],
};
const SNAPSHOT: TargetIndex = {
  meta: FULL_DATA.meta,
  targets: FULL_DATA.targets.map((item) => ({ ...item, location: null })),
};
const PREVIEW_TARGETS = [
  "//app:server",
  "//app/lib:all",
  "//app/cli:zeal_cli",
  "//app/lsp:completion_test",
  "//...",
];
const dayIndices = Array.from({ length: 30 }).keys();
type PreviewRecord = InvocationRecord & {
  workspace: string;
  cmd: string;
  pattern: string;
  startedAt: number;
};
const records: PreviewRecord[] = Array.from(dayIndices, (dayIndex) => {
  const dayDate = new Date(NOW);
  dayDate.setDate(dayDate.getDate() - dayIndex);
  const invocationCount = (dayIndex * 7 + 3) % 9;
  const invocationIndices = Array.from({ length: invocationCount }).keys();
  return Array.from(invocationIndices, (invocationIndex) => {
    const startedAt = dayDate.getTime() - invocationIndex * 600000;
    return {
      id: `preview-${dayIndex}-${invocationIndex}`,
      workspace: WORKSPACE,
      cmd: invocationIndex % 3 === 1 ? "test" : "build",
      pattern: PREVIEW_TARGETS[(dayIndex + invocationIndex) % PREVIEW_TARGETS.length],
      startedAt,
      // This record has an exit code but no finish time: counted, never timed.
      ...(invocationIndex === 4
        ? {}
        : { finishedAt: startedAt + 4180 + invocationIndex * 1200 + (dayIndex % 5) * 900 }),
      exitCode: invocationIndex === 2 ? 1 : invocationIndex === 5 ? 8 : 0,
    };
  });
}).flat();
const state: AppState = {
  besReady: true,
  scope: { include: ["//app/..."] },
  workspace: WORKSPACE,
  bazel: "8.7.0",
  ingest: "idle",
  snapshotFetchedAt: SNAPSHOT.meta.fetchedAt,
  lastError: null,
  activeInvocation: null,
  stopping: false,
  invocationNote: null,
  detailQuery: "idle",
};
let currentSnapshot: TargetIndex | null = SNAPSHOT;
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
function counter(name: string, field: string, values: number[]) {
  return values.map((value, i) => ({
    name,
    ts: i * 942000,
    ph: "C",
    args: { [field]: value },
  }));
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
      name: "system metrics",
      events: [
        ...counter(
          "CPU usage (Bazel)",
          "CPU cores",
          [1.1, 2.6, 3.8, 3.2, 2.5, 1.9, 2.7, 4.1, 3.5, 2.1, 1.4],
        ),
        ...counter("local action count", "actions", [0, 1, 5, 6, 7, 8, 7.8, 7.5, 6, 3, 0]),
        ...counter(
          "Memory usage (Bazel)",
          "memory",
          [620, 840, 1180, 1520, 1960, 2240, 2380, 2310, 2450, 2520, 2600],
        ),
      ],
    },
  ],
};

function readConsole(id: string, offset: number) {
  const bytes = new TextEncoder().encode(consoleOutput.get(id) ?? "");
  const data = Array.from(bytes.slice(offset, offset + 64 * 1024));
  return {
    data,
    offset: offset + data.length,
    done: state.activeInvocation !== id && offset + data.length === bytes.length,
  };
}

function writeConsole(id: string, data: unknown) {
  if (!Array.isArray(data)) throw new Error("Preview console input must be a byte array.");
  if (state.activeInvocation !== id) throw new Error("Preview process has ended");
  const text = new TextDecoder().decode(Uint8Array.from(data));
  const result = interpretConsoleInput(consoleInput.get(id) ?? "", text);
  consoleInput.set(id, result.input);
  consoleOutput.set(id, (consoleOutput.get(id) ?? "") + result.output);
  if (result.exitCode !== null) {
    const record = records.find((inv) => inv.id === id);
    if (!record) throw new Error("Preview invocation was not found.");
    record.finishedAt = Date.now();
    record.exitCode = result.exitCode;
    state.activeInvocation = null;
    void emit("app-state", structuredClone(state));
  }
  return data.length;
}

function getInvocation(id: unknown) {
  const record = records.find((inv) => inv.id === id);
  if (!record) throw new Error("Preview invocation not found");
  return {
    record,
    log: {
      status: "ok",
      data:
        consoleOutput.get(record.id) ??
        `INFO: ${record.cmd} ${record.pattern}\n${record.exitCode === 0 ? "INFO: Build completed successfully" : `ERROR: Invocation ended with exit ${record.exitCode}`}\n`,
    },
    bep: {
      status: "ok",
      data: {
        targets: FULL_DATA.targets
          .filter((target) => target.label === record.pattern)
          .map((target) => ({ label: target.label, ok: record.exitCode === 0 })),
        tests: [],
        ...(record.id === records[0]?.id
          ? {
              originalCommandLine: `bazel --output_user_root=/tmp/zeal-preview-root ${record.cmd} --config=dev --define='message=hello world' ${record.pattern}`,
              canonicalCommandLine: `bazel --output_user_root=/tmp/zeal-preview-root --batch ${record.cmd} --jobs=8 --keep_going --define='message=hello world' ${record.pattern}`,
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

function startInvocation(cmd: string, pattern: string) {
  const record: PreviewRecord = {
    id: `preview-${crypto.randomUUID()}`,
    workspace: WORKSPACE,
    cmd,
    pattern,
    startedAt: Date.now(),
  };
  records.unshift(record);
  state.activeInvocation = record.id;
  state.consoleInvocation = record.id;
  consoleOutput.set(
    record.id,
    `\x1b[32mINFO:\x1b[0m bazel ${record.cmd} ${record.pattern}\r\nPreview Console — sample data\r\n${record.cmd === "run" ? "Number: " : "Build completed.\r\n"}`,
  );
  if (record.cmd !== "run") {
    record.finishedAt = Date.now();
    record.exitCode = 0;
    state.activeInvocation = null;
  }
  void emit("app-state", structuredClone(state));
  return structuredClone(record);
}

function stopInvocation() {
  const id = state.activeInvocation;
  if (!id) return;
  const record = records.find((inv) => inv.id === id);
  state.stopping = true;
  void emit("app-state", structuredClone(state));
  if (record) {
    record.finishedAt = Date.now();
    record.exitCode = 8;
    state.activeInvocation = null;
    state.stopping = false;
    void emit("app-state", structuredClone(state));
  }
}

/** Installs mock core IPC for the development-only browser preview. */
export function installPreview() {
  if (!import.meta.env.DEV || isTauri()) throw new Error("Preview requires a development browser");
  if (new URLSearchParams(location.search).get("preview") === "empty") {
    state.workspace = null;
    state.snapshotFetchedAt = null;
    currentSnapshot = null;
  }
  // Every command the app invokes needs a handler; the Record type makes a
  // missing one a typecheck error instead of a runtime surprise.
  const handlers: Record<CommandName, (payload: Record<string, unknown> | undefined) => unknown> = {
    get_state: () => structuredClone(state),
    get_snapshot: () => structuredClone(currentSnapshot),
    get_target_detail: (payload): TargetDetail => {
      if (payload?.fetchedAt !== currentSnapshot?.meta.fetchedAt) {
        throw new Error("Target list changed; select the target again");
      }
      const label = String(payload?.label);
      const selected = FULL_DATA.targets.find((item) => item.label === label);
      if (!selected) throw new Error("Target not found");
      return structuredClone({
        target: selected,
        edges: FULL_DATA.edges.filter((edge) => edge.from === label || edge.to === label),
        files: FULL_DATA.files.filter((file) => file.owner === label),
        status: "ok",
      });
    },
    get_target_outputs: (payload): string[] => {
      if (payload?.fetchedAt !== currentSnapshot?.meta.fetchedAt) {
        throw new Error("Target list changed; select the target again");
      }
      const label = String(payload?.label);
      if (!SNAPSHOT.targets.some((item) => item.label === label)) {
        throw new Error("Target not found");
      }
      return FULL_DATA.files
        .filter((file) => file.owner === label && file.role === "out")
        .map((file) => `bazel-out/k8-fastbuild/bin/${file.path}`);
    },
    list_invocations: () => structuredClone(records),
    read_console: (payload) => readConsole(String(payload?.id), Number(payload?.offset ?? 0)),
    resize_console: () => {},
    write_console: (payload) => writeConsole(String(payload?.id), payload?.data),
    get_invocation: (payload) => getInvocation(payload?.id),
    get_invocation_profile: (payload) => {
      if (!records.some((inv) => inv.id === payload?.id)) {
        throw new Error("Preview invocation not found");
      }
      return { status: "ok", data: PROFILE };
    },
    // The app sends this command with the AnalyticsFilter shape.
    get_analytics: (payload) =>
      previewAnalytics(records, (payload?.filter ?? {}) as Partial<AnalyticsFilter>, NOW),
    open_workspace: (payload) => {
      state.workspace = String(payload?.path ?? WORKSPACE);
      state.scope = structuredClone(payload?.scope as AppState["scope"]);
      state.snapshotFetchedAt = SNAPSHOT.meta.fetchedAt;
      currentSnapshot = SNAPSHOT;
      return structuredClone(state);
    },
    refresh: () => void emit("app-state", structuredClone(state)),
    start_invocation: (payload) => startInvocation(String(payload?.cmd), String(payload?.pattern)),
    stop_invocation: () => stopInvocation(),
    run_query: (payload) => {
      if (bazelBusyReason(state)) {
        throw new Error("bazel is busy");
      }
      const id = `preview-query-${crypto.randomUUID()}`;
      const output = String(payload?.output ?? "label");
      const expr = String(payload?.expr ?? "");
      const labels = expr.includes("//")
        ? SNAPSHOT.targets.map((t) => t.label)
        : ["//app:server", "//app/lib:parser", "//tools:cli"];
      const finished = {
        exitCode: 0,
        stdout:
          output === "graph"
            ? `digraph mygraph {\n${labels
                .slice(0, 6)
                .map((l, i) => `  node${i} [label="${l}"];`)
                .join("\n")}\n${labels
                .slice(1, 6)
                .map((_, i) => `  node${i} -> node${i + 1};`)
                .join("\n")}\n}\n`
            : labels.join("\n") + "\n",
        truncated: false,
        stderrTail: "",
      };
      state.activeInvocation = id;
      state.stopping = false;
      void emit("app-state", structuredClone(state));
      return new Promise((resolve, reject) => {
        setTimeout(() => {
          const stopped = state.activeInvocation === id && state.stopping;
          if (state.activeInvocation === id) {
            state.activeInvocation = null;
            state.stopping = false;
            void emit("app-state", structuredClone(state));
          }
          if (stopped) reject(new Error("Query stopped"));
          else resolve(finished);
        }, 800);
      });
    },
  };
  mockIPC(
    (command, args) => {
      if (command === "plugin:dialog|open") return WORKSPACE;
      const handler = handlers[command as CommandName];
      if (!handler) throw new Error(`Unsupported preview command: ${command}`);
      return handler(args as Record<string, unknown> | undefined);
    },
    { shouldMockEvents: true },
  );
}
