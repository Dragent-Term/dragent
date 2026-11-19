/** Machine areas are distinct from the Agent / Terminal / Browser workspace tabs. */
export const MACHINE_VIEWS = [
  { id: "projects", label: "Project", hint: "Projects on this machine" },
  { id: "coordination", label: "Agent coordination", hint: "Agent coordination on this machine" },
  { id: "automation", label: "Automation", hint: "Automation on this machine" },
  { id: "review", label: "Code review", hint: "Code review on this machine" },
  { id: "tasks", label: "Tasks", hint: "Tasks on this machine" },
] as const;
export type MachineView = typeof MACHINE_VIEWS[number]["id"];
export function parseMachineView(value: unknown): MachineView {
  return MACHINE_VIEWS.find((item) => item.id === value)?.id ?? "projects";
}
