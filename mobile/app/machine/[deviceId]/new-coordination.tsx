import { useLocalSearchParams } from "expo-router";
import { MachineNewCoordinationScreen } from "../../../src/features/machines/MachineNewCoordinationScreen";
import { singleParam } from "../../../src/machines/routes";

/** A new agent coordination, started on the paired desktop. */
export default function MachineNewCoordinationRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[] }>();
  return <MachineNewCoordinationScreen deviceId={singleParam(params.deviceId)} />;
}
