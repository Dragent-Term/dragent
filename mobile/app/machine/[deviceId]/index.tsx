import { useLocalSearchParams } from "expo-router";
import { MachineScreen } from "../../../src/features/machines/MachineScreen";
import { singleParam } from "../../../src/machines/routes";

export default function MachineRoute() {
  const { deviceId: raw } = useLocalSearchParams<{ deviceId?: string | string[] }>();
  // The hook already decodes once; params carry raw ids, read raw.
  const deviceId = singleParam(raw);
  return <MachineScreen key={deviceId} deviceId={deviceId} />;
}
