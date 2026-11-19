import { useLocalSearchParams } from "expo-router";
import { MachineTaskDetailScreen } from "../../../../src/features/machines/MachineTaskDetailScreen";
import { singleParam } from "../../../../src/machines/routes";

/** Live task detail: the organisation's task through the paired desktop. */
export default function MachineTaskRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; taskId?: string | string[] }>();
  const deviceId = singleParam(params.deviceId);
  const taskId = singleParam(params.taskId);
  return <MachineTaskDetailScreen key={`${deviceId}\u0000${taskId}`} deviceId={deviceId} task={taskId} />;
}
