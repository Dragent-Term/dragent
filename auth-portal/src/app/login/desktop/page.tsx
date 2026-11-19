import { redirect } from "next/navigation";

/**
 * Compatibility redirect: /login/desktop → /login, query string intact.
 *
 * There is one login flow now, at /login. This route cannot simply be deleted:
 * the desktop app builds the sign-in URL at release time
 * (`dragent/app/src/auth/browser.rs`), so every already-installed build still
 * opens /login/desktop?scheme=…&state=…. Removing it would 404 sign-in for
 * everyone who has not updated.
 *
 * `scheme` and `state` must survive — they are the desktop hand-off, and
 * dropping them turns a linking attempt into an ordinary web sign-in that
 * silently never returns to the app.
 *
 * Safe to delete once no supported release points here.
 */
export default async function LoginDesktopRedirect({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;

  const qs = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (Array.isArray(value)) {
      for (const v of value) qs.append(key, v);
    } else if (value !== undefined) {
      qs.set(key, value);
    }
  }

  const query = qs.toString();
  redirect(query ? `/login?${query}` : "/login");
}
