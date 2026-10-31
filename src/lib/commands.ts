/** Names of the core's Tauri commands, in one place. The preview mock keys
 * its handlers on `CommandName`, so a new command fails typecheck until the
 * preview implements it too — the mock cannot silently fall behind. */
export const COMMANDS = {
  getState: "get_state",
  openWorkspace: "open_workspace",
  refresh: "refresh",
  openLocation: "open_location",
  listRuns: "list_runs",
  getRun: "get_run",
  getRunProfile: "get_run_profile",
  getAnalytics: "get_analytics",
  startRun: "start_run",
  stopRun: "stop_run",
  readConsole: "read_console",
  writeConsole: "write_console",
  resizeConsole: "resize_console",
} as const;

export type CommandName = (typeof COMMANDS)[keyof typeof COMMANDS];
