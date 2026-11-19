// The build script's guards run before any toolchain is touched.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { test } from "node:test";

const script = join(import.meta.dirname, "..", "..", "..", "scripts", "build-e2ee-rust.sh");
const run = (args: string[], extra: Record<string, string> = {}) => {
  const env: NodeJS.ProcessEnv = { ...process.env, ...extra };
  if (!("EAS_BUILD" in extra)) delete env.EAS_BUILD;
  if (!("EAS_BUILD_PROFILE" in extra)) delete env.EAS_BUILD_PROFILE;
  return spawnSync("bash", [script, ...args], { encoding: "utf8", env });
};

test("--vectors is refused inside an EAS build", () => {
  const r = run(["ios", "--vectors"], { EAS_BUILD_PROFILE: "production" });
  assert.equal(r.status, 2);
  assert.match(r.stderr, /never an EAS build/);
});

test("the eas command refuses to run outside EAS (it may install Rust)", () => {
  const r = run(["eas"]);
  assert.equal(r.status, 2);
  assert.match(r.stderr, /only runs inside an EAS build/);
});

test("unknown commands and options are refused", () => {
  assert.equal(run(["nope"]).status, 2);
  assert.equal(run(["pins", "--fast"]).status, 2);
});
