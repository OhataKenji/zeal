/** Lens Rail page destinations, in rail order. */
export const pages = [
  { id: "workspace", label: "Workspace" },
  { id: "targets", label: "Targets" },
  { id: "query", label: "Query" },
  { id: "history", label: "History" },
  { id: "analytics", label: "Analytics" },
] as const;

export type Page = (typeof pages)[number]["id"];
