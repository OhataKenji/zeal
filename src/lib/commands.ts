/** Names of the core's Tauri commands, in one place. The preview mock keys
 * its handlers on `CommandName`, so a new command fails typecheck until the
 * preview implements it too — the mock cannot silently fall behind. */
export const COMMANDS = {
  getState: "get_state",
  getWorkspaceInfo: "get_workspace_info",
  getModules: "get_modules",
  explainModule: "explain_module",
  getTargetDetail: "get_target_detail",
  readTarget: "read_target",
  openWorkspace: "open_workspace",
  listWorkspaces: "list_workspaces",
  refresh: "refresh",
  listInvocations: "list_invocations",
  getInvocation: "get_invocation",
  prepareInvocationTiming: "prepare_invocation_timing",
  getTestLog: "get_test_log",
  getAnalytics: "get_analytics",
  startInvocation: "start_invocation",
  stopInvocation: "stop_invocation",
  runQuery: "run_query",
  readConsole: "read_console",
  writeConsole: "write_console",
  resizeConsole: "resize_console",
} as const;

export type CommandName = (typeof COMMANDS)[keyof typeof COMMANDS];
