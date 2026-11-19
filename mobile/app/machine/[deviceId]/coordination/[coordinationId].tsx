import { useLocalSearchParams } from "expo-router";
import { MachineCoordinationDetailScreen } from "../../../../src/features/machines/MachineCoordinationDetailScreen";
import { singleParam } from "../../../../src/machines/routes";

/** Live coordination detail: real team, tasks and messages only — never sample. */
export default function MachineCoordinationRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; coordinationId?: string | string[] }>();
  // The hook already decodes once; params carry raw ids, read raw.
  const deviceId = singleParam(params.deviceId);
  const coordinationId = singleParam(params.coordinationId);
  return (
    <MachineCoordinationDetailScreen
      key={`${deviceId}\u0000${coordinationId}`}
      deviceId={deviceId}
      teamId={coordinationId}
    />
  );
}
