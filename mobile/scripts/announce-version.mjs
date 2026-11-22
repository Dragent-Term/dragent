#!/usr/bin/env node
// Announce a new phone build: phones running an older build show "A new
// version is available" (and, under --minimum, must update first).
//
//   pnpm version:announce ios 43 [--minimum 40] [--notes "Scrolling fixes"]
//   pnpm version:announce android 9 --url https://expo.dev/artifacts/eas/….apk
//   pnpm version:announce show
//
// Writes latest.json on the public repository's rolling `mobile-latest`
// GitHub pre-release (needs `gh` signed in with write access): build numbers
// and links only, never code (docs/design/e2ee.md §1.3). iOS links to the app
// in TestFlight unless --url says otherwise; Android needs the APK's --url.
//
// Phone builds from before the move read the releases CDN, so the file is
// also copied there while `aws` is signed in (skipped otherwise).
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = process.env.DRAGENT_RELEASES_REPO ?? "Dragent-Term/dragent";
const TAG = "mobile-latest";
const PUBLIC_URL = `https://github.com/${REPO}/releases/download/${TAG}/latest.json`;
const LEGACY_BUCKET = "dragent-releases-269235677023";
const LEGACY_KEY = "mobile/latest.json";
const TESTFLIGHT = "itms-beta://beta.itunes.apple.com/v1/app/6830027415";

const fail = (message) => {
  console.error(message);
  process.exit(2);
};
const run = (cmd, args) => execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
const gh = (args) => run("gh", args);

function current() {
  try {
    return JSON.parse(gh(["release", "download", TAG, "--repo", REPO, "--pattern", "latest.json", "--output", "-"]));
  } catch {
    return {};
  }
}

const [platform, buildArg, ...rest] = process.argv.slice(2);
if (platform === "show" || !platform) {
  console.log(JSON.stringify(current(), null, 2));
  process.exit(0);
}
if (platform !== "ios" && platform !== "android") fail("First say ios or android (or show).");
const latest = Number(buildArg);
if (!Number.isInteger(latest) || latest < 1) fail("Then the build number, e.g. 43.");
const flags = {};
for (let i = 0; i < rest.length; i += 2) {
  if (!rest[i]?.startsWith("--") || rest[i + 1] === undefined) fail(`Unexpected ${rest[i]}`);
  flags[rest[i].slice(2)] = rest[i + 1];
}

const manifest = current();
const before = manifest[platform] ?? {};
const minimum = flags.minimum !== undefined ? Number(flags.minimum) : (before.minimum ?? 0);
if (!Number.isInteger(minimum) || minimum < 0 || minimum > latest) fail("--minimum must be between 0 and the build number.");
const url = flags.url ?? (platform === "ios" ? TESTFLIGHT : before.url);
if (!url) fail("Android needs --url with the APK link.");
manifest[platform] = { latest, minimum, url, ...(flags.notes ? { notes: flags.notes } : {}) };

const file = join(mkdtempSync(join(tmpdir(), "dragent-version-")), "latest.json");
writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
try {
  gh(["release", "view", TAG, "--repo", REPO]);
} catch {
  gh([
    "release", "create", TAG, "--repo", REPO, "--prerelease",
    "--title", "Phone app version notice",
    "--notes", "The newest phone builds, read by the Dragent phone app to offer updates. Not a desktop release.",
  ]);
}
gh(["release", "upload", TAG, file, "--repo", REPO, "--clobber"]);
try {
  run("aws", [
    "s3", "cp", file, `s3://${LEGACY_BUCKET}/${LEGACY_KEY}`,
    "--content-type", "application/json",
    "--cache-control", "public, max-age=300",
  ]);
} catch {
  console.warn("(Skipped the CDN copy older phone builds read: aws isn't signed in.)");
}
console.log(readFileSync(file, "utf8"));
console.log(`Published: ${PUBLIC_URL}`);
