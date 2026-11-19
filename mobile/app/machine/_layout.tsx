import { Stack } from "expo-router";

/** Live machine routes: real relay data only, never sample content. */
export default function MachineLayout() {
  return <Stack screenOptions={{ headerShown: false }} />;
}
