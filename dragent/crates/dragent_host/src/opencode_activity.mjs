// Invocation-local lifecycle bridge. Replaced at launch, never installed globally.
export const DragentSession = async ({ client }) => {
  const helper = __DRAGENT_HELPER_ARGS__;
  const roots = new Map();
  let selected = null;
  let last = null;
  let pending = Promise.resolve();
  const enqueue = (fn) => {
    pending = pending.then(fn).catch(() => {});
    return pending;
  };
  const isRoot = async (id) => {
    if (!id) return false;
    if (roots.has(id)) return roots.get(id);
    const result = await client.session.get({ path: { id } });
    if (!result.data) return false;
    const root = !result.data.parentID;
    roots.set(id, root);
    return root;
  };
  const report = async (id, event, select = false) => {
    if (!(await isRoot(id))) return;
    if (select || !selected) { selected = id; last = null; }
    if (id !== selected || last === event) return;
    if (last === "Failed" && event === "Stop") return;
    if (last === "UserPromptSubmit" && event === "Start") return;
    const child = Bun.spawn({ cmd: helper,
      stdin: new TextEncoder().encode(JSON.stringify({ session_id: id, hook_event_name: event })),
      stdout: "ignore", stderr: "ignore" });
    if (await child.exited === 0) last = event;
  };
  return {
    event: ({ event }) => enqueue(async () => {
      const p = event.properties || {};
      const id = p.sessionID || p.info?.id;
      if (event.type === "session.created") {
        roots.set(id, !p.info?.parentID);
        if (!selected) await report(id, "SessionStart", true);
      } else if (event.type === "tui.session.select") {
        await report(id, "SessionStart", true);
      } else if (event.type === "session.status") {
        if (p.status?.type === "idle") await report(id, "Stop");
        if (p.status?.type === "busy" || p.status?.type === "retry") await report(id, "Start");
      } else if (event.type === "session.idle") {
        await report(id, "Stop");
      } else if (event.type === "session.busy") {
        await report(id, "Start");
      } else if (event.type === "permission.asked" || event.type === "question.asked") {
        await report(id, "PermissionRequest");
      } else if (event.type === "permission.replied" || event.type === "question.replied" || event.type === "question.rejected") {
        await report(id, "Start");
      } else if (event.type === "session.error") {
        await report(id, "Failed");
      } else if (event.type === "session.deleted" && id === selected) {
        await report(id, "SessionEnd");
        selected = null;
        roots.delete(id);
      }
    }),
    "chat.message": (input) => enqueue(() => report(input.sessionID, "UserPromptSubmit", input.sessionID !== selected)),
    "command.execute.before": (input) => enqueue(() => report(input.sessionID, "SessionStart", input.sessionID !== selected)),
    "permission.ask": (input, output) => enqueue(async () => {
      if (output.status === "ask") await report(input.sessionID || selected, "PermissionRequest");
    }),
  };
};
