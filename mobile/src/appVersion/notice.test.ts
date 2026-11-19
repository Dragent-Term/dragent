import assert from "node:assert/strict";
import { test } from "node:test";
import { allowedUpdateUrl, buildNumber, noticeFor, parseRelease } from "./notice";

const manifest = {
  ios: { latest: 43, minimum: 40, url: "itms-beta://beta.itunes.apple.com/v1/app/6830027415", notes: " Scrolling fixes " },
  android: { latest: 9, url: "https://expo.dev/artifacts/eas/abc.apk" },
};

test("reads each platform's release", () => {
  assert.deepEqual(parseRelease(manifest, "ios"), {
    latest: 43, minimum: 40, url: "itms-beta://beta.itunes.apple.com/v1/app/6830027415", notes: "Scrolling fixes",
  });
  assert.deepEqual(parseRelease(manifest, "android"), { latest: 9, minimum: 0, url: "https://expo.dev/artifacts/eas/abc.apk", notes: null });
});

test("an older build is offered the update; a build under the minimum must update", () => {
  const ios = parseRelease(manifest, "ios");
  assert.equal(noticeFor(ios, 43).kind, "current");
  assert.equal(noticeFor(ios, 44).kind, "current");
  assert.equal(noticeFor(ios, 42).kind, "available");
  assert.equal(noticeFor(ios, 40).kind, "available");
  assert.equal(noticeFor(ios, 39).kind, "required");
});

test("unknown build or missing release says nothing", () => {
  assert.equal(noticeFor(null, 1).kind, "current");
  assert.equal(noticeFor(parseRelease(manifest, "ios"), null).kind, "current");
});

test("links may only lead to the stores, TestFlight and our build host", () => {
  for (const ok of ["https://apps.apple.com/app/id1", "https://testflight.apple.com/join/x", "https://play.google.com/store/apps/details?id=com.dragent.mobile", "https://expo.dev/artifacts/eas/a.apk", "itms-beta://x", "itms-apps://x", "market://details?id=x"]) {
    assert.equal(allowedUpdateUrl(ok), true, ok);
  }
  for (const bad of ["http://expo.dev/a.apk", "https://evil.example/a.apk", "https://expo.dev.evil.example/a", "https://user:pw@expo.dev/a", "javascript:alert(1)", "not a url"]) {
    assert.equal(allowedUpdateUrl(bad), false, bad);
  }
  assert.equal(parseRelease({ ios: { latest: 2, url: "https://evil.example/x" } }, "ios"), null);
});

test("malformed entries are ignored", () => {
  assert.equal(parseRelease({ ios: { latest: "43", url: "itms-beta://x" } }, "ios"), null);
  assert.equal(parseRelease({ ios: { latest: 3, minimum: 5, url: "itms-beta://x" } }, "ios"), null);
  assert.equal(parseRelease({ ios: { latest: -1, url: "itms-beta://x" } }, "ios"), null);
  assert.equal(parseRelease(null, "ios"), null);
  assert.equal(parseRelease([], "android"), null);
});

test("build numbers come from the OS as digits", () => {
  assert.equal(buildNumber("43"), 43);
  assert.equal(buildNumber(" 9 "), 9);
  assert.equal(buildNumber("1.0"), null);
  assert.equal(buildNumber(null), null);
});
