import type { PreviewSnapshot } from "./types";

/** Original fixed samples. No scheduler, coordinator or model runs here. */
export function createMachineFixtures(): Pick<PreviewSnapshot, "coordinations" | "automations" | "automationRuns" | "reviews"> {
  return {
    coordinations: [{
      id: "navigation-team", desktopId: "preview-mac", projectId: "lumen-web",
      title: "Refine workspace navigation",
      goal: "Keep projects, branches and open tabs easy to follow on every screen size.",
      status: "running", leadTabId: "preview-navigation-agent", parallelism: 2, autoStart: true,
      plan: [
        { id: "structure", title: "Define the project and workspace structure", status: "done", dependsOn: [], tabId: "preview-navigation-codex" },
        { id: "navigation", title: "Build the workspace navigation", status: "working", dependsOn: ["structure"], tabId: "preview-navigation-codex" },
        { id: "empty-states", title: "Polish empty states", status: "working", dependsOn: ["structure"], tabId: "preview-empty-states-agent" },
        { id: "review", title: "Review the complete flow", status: "pending", dependsOn: ["navigation", "empty-states"], tabId: null },
      ],
      messages: [
        { id: "planning", from: "system", body: "Sample plan prepared. Two independent parts can run side by side.", at: Date.UTC(2026, 0, 15, 9, 40) },
        { id: "lead-note", from: "lead", body: "Navigation and empty states have separate workspaces. The final review waits for both.", at: Date.UTC(2026, 0, 15, 9, 45) },
      ],
    }, {
      id: "retry-team", desktopId: "preview-linux", projectId: "relay-api",
      title: "Review retry behavior", goal: "Keep retries bounded and explain the retry window.",
      status: "done", leadTabId: "preview-retries-agent", parallelism: 1, autoStart: false,
      plan: [{ id: "retry-check", title: "Check retry bounds", status: "done", dependsOn: [], tabId: "preview-retries-agent" }],
      messages: [{ id: "finished", from: "lead", body: "Sample review complete. The result is available in the retry workspace.", at: Date.UTC(2026, 0, 15, 9) }],
    }],
    automations: [{
      id: "navigation-check", desktopId: "preview-mac", projectId: "lumen-web",
      workspaceId: "preview-navigation", agentTabId: "preview-navigation-agent",
      name: "Navigation check", prompt: "Review project and workspace navigation. Summarize any confusing transitions and suggest a focused improvement.",
      schedule: "Weekdays at 09:00", timeZone: "Africa/Lagos", enabled: true, workspaceMode: "pinned",
    }, {
      id: "docs-check", desktopId: "preview-mac", projectId: "field-notes",
      workspaceId: "preview-quick-start", agentTabId: "preview-quick-start-agent",
      name: "Documentation check", prompt: "Review the quick start for clarity and summarize any missing steps.",
      schedule: "Mondays at 10:00", timeZone: "Africa/Lagos", enabled: false, workspaceMode: "pinned",
    }],
    automationRuns: [{
      id: "navigation-sample-run", automationId: "navigation-check", workspaceId: "preview-navigation",
      at: Date.UTC(2026, 0, 15, 8), status: "succeeded",
      output: "Sample result: the project opens its workspaces and every workspace shows its own branch. No scheduler or model ran.",
    }],
    reviews: [],
  };
}
