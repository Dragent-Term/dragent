import { Stack, useLocalSearchParams } from "expo-router";
import { MachineProvider } from "../../../src/machines/MachineContext";
import { singleParam } from "../../../src/machines/routes";

/**
 * Machine boundary: owns ONE targeted socket for the selected device and
 * shares it (plus the catalog subscription) with project/workspace/tab
 * children through MachineProvider. Keyed by device so switching machines
 * remounts — never showing the previous machine's data.
 */
export default function MachineDeviceLayout() {
  const { deviceId: raw } = useLocalSearchParams<{ deviceId?: string | string[] }>();
  // The hook already decodes once; params carry raw ids, read raw.
  const deviceId = singleParam(raw);
  return (
    <MachineProvider key={deviceId} deviceId={deviceId}>
      <Stack screenOptions={{ headerShown: false }} />
    </MachineProvider>
  );
}
