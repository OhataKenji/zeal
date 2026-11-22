// Browser-only development preview. Never loaded by a production build.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type {
  AppState,
  Edge,
  FileEntry,
  InvocationRecord,
  ModulesSnapshot,
  OwnedInvocation,
  Profile,
  Target,
  TargetActions,
  TargetDetail,
  TargetIndex,
  TargetOutputs,
  WorkspaceState,
} from "./lib/types";
import type { QueryOutput, QueryRequest } from "./lib/query.ts";
import type { AnalyticsFilter } from "./lib/analytics";
import { previewAnalytics } from "./preview/analytics.ts";
import { interpretConsoleInput } from "./preview/console.ts";
import type { CommandName } from "./lib/commands.ts";

const WORKSPACE = "/demo/cool-mono";
const MODULES: ModulesSnapshot = {
  rootKey: "<root>",
  rootName: "cool_mono",
  warning: null,
  modules: [
    {
      key: "rules_cc@0.0.17",
      name: "rules_cc",
      version: "0.0.17",
      direct: true,
      requiredBy: ["<root>"],
      dependencies: ["platforms@0.0.11"],
    },
    {
      key: "platforms@0.0.11",
      name: "platforms",
      version: "0.0.11",
      direct: false,
      requiredBy: ["rules_cc@0.0.17"],
      dependencies: [],
    },
  ],
};
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
      workspace:
        invocationIndex === 2
          ? "/workspaces/build-tools"
          : (dayIndex + invocationIndex) % 2 === 0
            ? WORKSPACE
            : `${WORKSPACE}-worktree`,
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
  revision: 0,
  besError: null,
  besEndpoint: "grpc://127.0.0.1:4718",
  bazel: "8.7.0",
  lastWorkspace: WORKSPACE,
  workspaces: [
    {
      workspace: WORKSPACE,
      scope: { include: ["//app/..."] },
    },
  ],
  invocations: [],
};

function emitState() {
  state.revision += 1;
  void emit("app-state", structuredClone(state));
}

function wsEntry(workspace: unknown): WorkspaceState {
  const id = String(workspace ?? "");
  const entry = state.workspaces.find((item) => item.workspace === id);
  if (!entry) throw new Error(`Preview workspace is not open: ${id}`);
  return entry;
}

function ownedFor(id: string): OwnedInvocation | undefined {
  return state.invocations.find((entry) => entry.id === id);
}
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

function readConsole(workspace: string, id: string, offset: number) {
  wsEntry(workspace);
  const owned = ownedFor(id);
  if (!owned || !owned.console) throw new Error("Preview invocation not found");
  if (owned.workspace !== workspace) {
    throw new Error("Preview invocation belongs to another workspace");
  }
  const bytes = new TextEncoder().encode(consoleOutput.get(id) ?? "");
  const data = Array.from(bytes.slice(offset, offset + 64 * 1024));
  return {
    data,
    offset: offset + data.length,
  };
}

function writeConsole(workspace: string, id: string, data: unknown) {
  wsEntry(workspace);
  const owned = ownedFor(id);
  if (owned && owned.workspace !== workspace) {
    throw new Error("Preview invocation belongs to another workspace");
  }
  if (!owned) throw new Error("Preview process has ended");
  if (!owned.console || !owned.stoppable || owned.stopping) {
    throw new Error("Preview console is not writable");
  }
  if (!Array.isArray(data)) throw new Error("Preview console input must be a byte array.");
  const text = new TextDecoder().decode(Uint8Array.from(data));
  const result = interpretConsoleInput(consoleInput.get(id) ?? "", text);
  consoleInput.set(id, result.input);
  consoleOutput.set(id, (consoleOutput.get(id) ?? "") + result.output);
  if (result.exitCode !== null) finishOwned(id);
  return data.length;
}

function getInvocation(workspace: string, id: unknown) {
  const record = records.find((inv) => inv.id === id && inv.workspace === workspace);
  if (!record) throw new Error("Preview invocation not found");
  return {
    record,
    log: {
      status: "ok",
      data: `INFO: ${record.cmd} ${record.pattern}\n${record.exitCode === 0 ? "INFO: Build completed successfully" : `ERROR: Invocation ended with exit ${record.exitCode}`}\n`,
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

function finishOwned(id: string) {
  state.invocations = state.invocations.filter((entry) => entry.id !== id);
  consoleOutput.delete(id);
  consoleInput.delete(id);
  emitState();
}

function startInvocation(workspace: string, cmd: string, pattern: string) {
  wsEntry(workspace);
  const id = `preview-${crypto.randomUUID()}`;
  const now = Date.now();
  // The BEP build result exists independently of the owned process: a `run`
  // program may outlive it, and its own output is never BEP history.
  const record: PreviewRecord = {
    id,
    workspace,
    cmd,
    pattern,
    startedAt: now,
    finishedAt: now,
    exitCode: 0,
  };
  records.unshift(record);
  const owned: OwnedInvocation = {
    id,
    workspace,
    cmd,
    pattern,
    startedAt: record.startedAt,
    stoppable: true,
    stopping: false,
    console: true,
  };
  state.invocations.unshift(owned);
  consoleOutput.set(
    id,
    `\x1b[32mINFO:\x1b[0m bazel ${record.cmd} ${record.pattern}\r\nPreview Console — sample data\r\n${record.cmd === "run" ? "Number: " : "Build completed.\r\n"}`,
  );
  if (cmd !== "run") {
    // build/test finish on their own; `run` stays live until input or Stop.
    setTimeout(() => finishOwned(id), 800);
  }
  emitState();
  void emit("invocations-changed");
  return id;
}

function stopInvocation(workspace: string, id: string) {
  wsEntry(workspace);
  const owned = ownedFor(id);
  if (owned && owned.workspace !== workspace) {
    throw new Error("Preview invocation belongs to another workspace");
  }
  if (!owned?.stoppable) return;
  owned.stopping = true;
  emitState();
  finishOwned(id);
}

function actionMnemonic(path: string): string {
  const name = path.split("/").pop() ?? path;
  if (name.endsWith("-src.jar") || name.endsWith(".srcjar")) return "SourceJar";
  if (name.endsWith("_deploy.jar")) return "DeployJar";
  if (name.endsWith(".jar")) return "Javac";
  return "SingleJar";
}

/** Sample actions for one target, built from the demo workspace files. */
function previewActions(label: string): TargetActions {
  const inputs = FULL_DATA.files
    .filter((file) => file.owner === label && file.role === "srcs")
    .map((file) => file.path);
  const actions = FULL_DATA.files
    .filter((file) => file.owner === label && file.role === "out")
    .map((file) => {
      const output = `bazel-out/k8-fastbuild/bin/${file.path}`;
      const mnemonic = actionMnemonic(file.path);
      return {
        output,
        mnemonic,
        owner: file.owner,
        arguments: [
          mnemonic.toLowerCase(),
          "--output",
          output,
          "--define=GREETING=hello world",
          ...inputs.slice(0, 2),
        ],
        inputs,
        inputsTruncated: false,
        outputs: [output],
        environmentVariables: [
          { key: "PATH", value: "/usr/bin:/bin" },
          { key: "LC_ALL", value: "C" },
        ],
        configuration: {
          mnemonic: "k8-fastbuild",
          platformName: "@local_config_platform//:host",
          checksum: "0f3c9a1e7b2d4f60",
          isTool: false,
        },
        executionPlatform: "@local_config_platform//:host",
      };
    });
  return { actions: actions.slice(0, 100), truncated: actions.length > 100 };
}

/** Sample `run_query` reply covering the three graphs and output modes. */
function previewQueryOutput({ kind, output, expr, flags }: QueryRequest): QueryOutput {
  // `--config=<name>` shows up in configured output the way Bazel reports it.
  const config = /--config=(\S+)/.exec(flags)?.[1] ?? "k8-fastbuild";
  const labels = SNAPSHOT.targets.map((t) => t.label);
  let stdout = "";
  let actions: TargetActions | null = null;
  switch (output) {
    case "jsonproto":
    case "text": {
      const set = previewActions(SNAPSHOT.targets.some((t) => t.label === expr) ? expr : PARSER);
      if (output === "jsonproto") {
        actions = set;
      } else {
        stdout = set.actions
          .slice(0, 10)
          .map(
            (a) =>
              `action '${a.mnemonic}'\n  Outputs: "${a.output}"\n  Owner: ${a.owner}\n  Inputs: ${a.inputs.length} file(s)\n`,
          )
          .join("\n");
      }
      break;
    }
    case "transitions":
      stdout =
        labels
          .slice(0, 10)
          .flatMap((label, i) => [
            `${i % 2 === 0 ? "target_platform" : "target_configuration_transition"} -> ${label} (${config})`,
            `  ${label} (${config})`,
          ])
          .join("\n") + "\n";
      break;
    case "graph":
      stdout = `digraph mygraph {\n${FULL_DATA.edges
        .filter((e) => e.from === PARSER)
        .slice(0, 6)
        .map((e) => `  ${JSON.stringify(e.from)} -> ${JSON.stringify(e.to)};`)
        .join("\n")}\n}\n`;
      break;
    default:
      stdout =
        (kind === "cquery" ? labels.map((label) => `${label} (${config})`) : labels).join("\n") +
        "\n";
  }
  return { exitCode: 0, stdout, truncated: false, stderrTail: "", actions };
}

/** Installs mock core IPC for the development-only browser preview. */
export function installPreview() {
  if (!import.meta.env.DEV || isTauri()) throw new Error("Preview requires a development browser");
  if (new URLSearchParams(location.search).get("preview") === "empty") {
    state.lastWorkspace = null;
    state.workspaces = [];
  }
  function candidates(): string[] {
    const counts = new Map<string, { count: number; last: number }>();
    for (const inv of records
      .toSorted((a, b) => b.startedAt - a.startedAt || a.id.localeCompare(b.id))
      .slice(0, 100)) {
      const rank = counts.get(inv.workspace);
      if (rank) rank.count++;
      else counts.set(inv.workspace, { count: 1, last: inv.startedAt });
    }
    return [...counts]
      .sort(([a, x], [b, y]) => y.count - x.count || y.last - x.last || a.localeCompare(b))
      .slice(0, 5)
      .map(([path]) => path);
  }
  // Every command the app invokes needs a handler; the Record type makes a
  // missing one a typecheck error instead of a runtime surprise.
  const handlers: Record<CommandName, (payload: Record<string, unknown> | undefined) => unknown> = {
    get_state: () => structuredClone(state),
    get_modules: (payload) => {
      wsEntry(payload?.workspace);
      return structuredClone(MODULES);
    },
    explain_module: (payload) => {
      wsEntry(payload?.workspace);
      const key = payload?.moduleKey;
      if (
        typeof key !== "string" ||
        !key ||
        key.length > 256 ||
        key.startsWith("-") ||
        /[^a-zA-Z0-9._+@~-]/.test(key)
      ) {
        throw new Error("Invalid module key");
      }
      const node = MODULES.modules.find((item) => item.key === key);
      if (!node) throw new Error(`Unknown module: ${key}`);
      return {
        text: [
          MODULES.rootKey,
          ...node.requiredBy.filter((parent) => parent !== MODULES.rootKey),
          key,
        ].join(" -> "),
        warning: null,
      };
    },
    get_workspace_info: (payload) => {
      const entry = wsEntry(payload?.workspace);
      const ob = `/private/var/tmp/_bazel_dev/preview`;
      return {
        workspace: entry.workspace,
        release: "release 8.7.0",
        outputBase: ob,
        executionRoot: `${ob}/execroot/_main`,
        bazelBin: `${ob}/execroot/_main/bazel-out/darwin_arm64-fastbuild/bin`,
        bazelTestlogs: `${ob}/execroot/_main/bazel-out/darwin_arm64-fastbuild/testlogs`,
        repositoryCache: "/Users/dev/Library/Caches/bazel/repo_cache",
      };
    },
    get_target_detail: (payload): TargetDetail => {
      wsEntry(payload?.workspace);
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
    get_target_outputs: (payload): TargetOutputs => {
      wsEntry(payload?.workspace);
      const label = String(payload?.label);
      if (!SNAPSHOT.targets.some((item) => item.label === label)) {
        throw new Error("Target not found");
      }
      const paths = FULL_DATA.files
        .filter((file) => file.owner === label && file.role === "out")
        .map((file) => `bazel-out/k8-fastbuild/bin/${file.path}`);
      return { paths: paths.slice(0, 10), truncated: paths.length > 10 };
    },
    get_target_actions: (payload): TargetActions => {
      wsEntry(payload?.workspace);
      const label = String(payload?.label);
      if (!SNAPSHOT.targets.some((item) => item.label === label)) {
        throw new Error("Target not found");
      }
      return previewActions(label);
    },
    list_invocations: (payload) =>
      structuredClone(records.filter((inv) => inv.workspace === String(payload?.workspace ?? ""))),
    read_console: (payload) =>
      readConsole(
        String(payload?.workspace ?? ""),
        String(payload?.id),
        Number(payload?.offset ?? 0),
      ),
    resize_console: () => {},
    write_console: (payload) =>
      writeConsole(String(payload?.workspace ?? ""), String(payload?.id), payload?.data),
    get_invocation: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      return getInvocation(workspace, payload?.id);
    },
    get_invocation_profile: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      if (!records.some((inv) => inv.id === payload?.id && inv.workspace === workspace)) {
        throw new Error("Preview invocation not found");
      }
      return { status: "ok", data: PROFILE };
    },
    // The app sends this command with the AnalyticsFilter shape.
    get_analytics: (payload) => {
      const workspace = String(payload?.workspace ?? "");
      wsEntry(workspace);
      return previewAnalytics(
        records.filter((inv) => inv.workspace === workspace),
        (payload?.filter ?? {}) as Partial<AnalyticsFilter>,
        NOW,
      );
    },
    list_workspaces: () => structuredClone(candidates()),
    open_workspace: (payload) => {
      const workspace = String(payload?.path ?? WORKSPACE);
      const scope = structuredClone(payload?.scope) as WorkspaceState["scope"];
      let entry = state.workspaces.find((item) => item.workspace === workspace);
      if (!entry) {
        entry = { workspace, scope };
        state.workspaces.push(entry);
      } else {
        entry.scope = scope;
      }
      state.lastWorkspace = workspace;
      emitState();
      return { workspace, state: structuredClone(state) };
    },
    refresh: (payload) => {
      wsEntry(payload?.workspace);
      return { snapshot: structuredClone(SNAPSHOT), error: null };
    },
    start_invocation: (payload) =>
      startInvocation(
        String(payload?.workspace ?? ""),
        String(payload?.cmd),
        String(payload?.pattern),
      ),
    stop_invocation: (payload) =>
      stopInvocation(String(payload?.workspace ?? ""), String(payload?.id)),
    run_query: (payload) => {
      const request = payload?.request as QueryRequest;
      wsEntry(request.workspace);
      const { workspace, kind, expr } = request;
      const finished = previewQueryOutput(request);
      const owned: OwnedInvocation = {
        id: `preview-query-${crypto.randomUUID()}`,
        workspace,
        cmd: kind,
        pattern: expr,
        startedAt: Date.now(),
        stoppable: true,
        stopping: false,
        console: false,
      };
      state.invocations.unshift(owned);
      emitState();
      return new Promise((resolve, reject) => {
        setTimeout(() => {
          const stopped = owned.stopping;
          if (!stopped) finishOwned(owned.id);
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
