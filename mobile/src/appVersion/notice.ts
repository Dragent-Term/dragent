/**
 * "A new version of Dragent is available." The phone reads a small public
 * file listing the newest build for each platform and where to get it. It
 * carries numbers and links only, never code: new code arrives only through
 * signed store and TestFlight builds (docs/design/e2ee.md §1.3), so this file
 * can at worst point at the wrong page, and only a few known pages are
 * allowed.
 *
 *   { "ios":     { "latest": 43, "minimum": 40, "url": "itms-beta://…", "notes": "…" },
 *     "android": { "latest": 9,  "minimum": 0,  "url": "https://expo.dev/artifacts/eas/….apk" } }
 *
 * `latest` newer than this build → a card offering the update. `minimum`
 * newer than this build → this build can no longer work; a full screen asks
 * for the update first.
 */

export type Platform = "ios" | "android";

export interface PlatformRelease {
  readonly latest: number;
  readonly minimum: number;
  readonly url: string;
  readonly notes: string | null;
}

export type VersionNotice =
  | { readonly kind: "current" }
  | { readonly kind: "available"; readonly release: PlatformRelease }
  | { readonly kind: "required"; readonly release: PlatformRelease };

/** Where an update link may lead: the stores, TestFlight, and our build host. */
const ALLOWED_HOSTS = new Set(["apps.apple.com", "testflight.apple.com", "play.google.com", "expo.dev"]);
const ALLOWED_SCHEMES = new Set(["itms-beta:", "itms-apps:", "market:"]);

export function allowedUpdateUrl(raw: string): boolean {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return false;
  }
  if (ALLOWED_SCHEMES.has(url.protocol)) return true;
  return url.protocol === "https:" && ALLOWED_HOSTS.has(url.hostname) && !url.username && !url.password;
}

function count(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

/** One platform's entry, or null when it is missing or not trustworthy. */
export function parseRelease(manifest: unknown, platform: Platform): PlatformRelease | null {
  if (!manifest || typeof manifest !== "object") return null;
  const entry = (manifest as Record<string, unknown>)[platform];
  if (!entry || typeof entry !== "object") return null;
  const fields = entry as Record<string, unknown>;
  const latest = count(fields.latest);
  const minimum = count(fields.minimum ?? 0);
  const url = typeof fields.url === "string" ? fields.url.trim() : "";
  if (latest === null || minimum === null || minimum > latest || !allowedUpdateUrl(url)) return null;
  const notes = typeof fields.notes === "string" && fields.notes.trim() ? fields.notes.trim().slice(0, 280) : null;
  return { latest, minimum, url, notes };
}

/** What to tell someone running build `build` (the store build number). */
export function noticeFor(release: PlatformRelease | null, build: number | null): VersionNotice {
  if (!release || build === null || !Number.isFinite(build)) return { kind: "current" };
  if (build < release.minimum) return { kind: "required", release };
  if (build < release.latest) return { kind: "available", release };
  return { kind: "current" };
}

/** The native build number as the OS reports it ("43"), or null. */
export function buildNumber(raw: string | null | undefined): number | null {
  if (!raw || !/^\d+$/.test(raw.trim())) return null;
  return Number(raw.trim());
}
