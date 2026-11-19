/**
 * Live Tasks reads, polled through the paired desktop with the machine
 * resource (same eligibility, holds and retry as live coordination).
 * Tasks change less often than agent teams, so they poll every 15 s.
 */
import { useCallback } from "react";
import type { RelayRpcClient } from "../transport/relayRpcClient";
import { useMachineResource } from "../coordination/useLiveCoordination";
import { getTask, listTasks } from "./rpc";
import type { TaskDetailResult, TaskListFilter, TaskListResult } from "./types";

export const TASKS_POLL_MS = 15_000;

/** `assignee`: "" for everyone, `me`, `none` or a member's id. */
export function useLiveTaskList(input: { status: TaskListFilter; q: string; assignee: string; label: string }) {
  const { status, q, assignee, label } = input;
  const fetch = useCallback(
    (client: RelayRpcClient) =>
      listTasks(client, { status, q, ...(assignee ? { assignee } : {}), ...(label ? { label } : {}) }),
    [status, q, assignee, label],
  );
  return useMachineResource<TaskListResult>({
    scope: `tasks\u0000${status}\u0000${assignee}\u0000${label}\u0000${q}`,
    fetch,
    pollMs: TASKS_POLL_MS,
  });
}

export function useLiveTaskDetail(task: string) {
  const fetch = useCallback((client: RelayRpcClient) => getTask(client, task), [task]);
  return useMachineResource<TaskDetailResult>({
    scope: `task\u0000${task}`,
    fetch,
    pollMs: TASKS_POLL_MS,
  });
}
