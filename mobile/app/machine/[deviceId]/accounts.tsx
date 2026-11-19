import { useLocalSearchParams } from "expo-router";
import { MachineAccountsScreen } from "../../../src/features/machines/MachineAccountsScreen";
import { singleParam } from "../../../src/machines/routes";

/** The desktop's agent accounts (Claude Code, Codex). */
export default function MachineAccountsRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[] }>();
  return <MachineAccountsScreen deviceId={singleParam(params.deviceId)} />;
}
