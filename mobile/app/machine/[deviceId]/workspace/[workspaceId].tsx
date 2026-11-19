import { useLocalSearchParams } from "expo-router";
import { MachineWorkspaceScreen } from "../../../../src/features/machines/MachineWorkspaceScreen";
import { singleParam } from "../../../../src/machines/routes";

export default function MachineWorkspaceRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; workspaceId?: string | string[] }>();
  const deviceId = singleParam(params.deviceId);
  const workspaceId = singleParam(params.workspaceId);
  return (
    <MachineWorkspaceScreen key={`${deviceId}\u0000${workspaceId}`} deviceId={deviceId} workspaceId={workspaceId} />
  );
}
