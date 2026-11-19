import { useLocalSearchParams } from "expo-router";
import { MachineProjectScreen } from "../../../../src/features/machines/MachineProjectScreen";
import { singleParam } from "../../../../src/machines/routes";

export default function MachineProjectRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; projectId?: string | string[] }>();
  const deviceId = singleParam(params.deviceId);
  const projectId = singleParam(params.projectId);
  return <MachineProjectScreen key={`${deviceId}\u0000${projectId}`} deviceId={deviceId} projectId={projectId} />;
}
