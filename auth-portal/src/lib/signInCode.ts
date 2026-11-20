/**
 * The short code that ties a browser sign-in to the Dragent app waiting for
 * it. Both sides derive it from the sign-in's state (the app shows it while
 * it waits; the browser shows it before approving), so a match means the
 * person approving is looking at the app that started this sign-in, not at
 * a link someone else sent them. Must match dragent/app/src/auth/code.rs.
 */
const ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

export async function signInCode(state: string): Promise<string> {
  const digest = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(state))
  );
  let code = "";
  for (let i = 0; i < 6; i++) code += ALPHABET[digest[i] % ALPHABET.length];
  return `${code.slice(0, 3)}-${code.slice(3)}`;
}
