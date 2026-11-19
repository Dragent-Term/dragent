/** Official-style platform glyphs (lucide doesn't ship these trademarks). */

export function AppleLogo({
  size = 16,
  className = "",
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M17.05 12.53c-.02-2.02 1.65-2.99 1.72-3.04-0.94-1.37-2.4-1.56-2.92-1.58-1.24-.13-2.42.73-3.05.73-.63 0-1.6-.71-2.63-.69-1.35.02-2.6.79-3.3 2-1.4 2.44-.36 6.05 1 8.03.67.97 1.47 2.06 2.5 2.02 1-.04 1.38-.65 2.6-.65 1.21 0 1.55.65 2.6.63 1.08-.02 1.76-.99 2.42-1.96.76-1.12 1.08-2.2 1.1-2.26-.02-.01-2.11-.81-2.13-3.2zM15.06 6.5c.55-.67.93-1.6.82-2.53-.8.03-1.77.53-2.34 1.2-.51.59-.96 1.54-.84 2.44.89.07 1.8-.45 2.36-1.11z" />
    </svg>
  );
}

export function WindowsLogo({
  size = 16,
  className = "",
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M3 5.4 10.2 4.4v6.9H3V5.4zm0 13.2 7.2 1v-6.8H3v5.8zM11.1 4.27 21 3v8.3h-9.9V4.27zm0 15.46L21 21v-8.2h-9.9v6.93z" />
    </svg>
  );
}

export function LinuxLogo({
  size = 16,
  className = "",
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M12 2c-1.7 0-3 1.5-3 3.3 0 1 .1 1.8.1 2.6 0 .8-.6 1.5-1.3 2.6-.8 1.2-1.8 2.6-1.8 4.4 0 .5.1.9.3 1.3-.3.3-.5.7-.5 1.2 0 .6.3 1 .8 1.3.5.3 1.2.4 2 .6.7.2 1.3.5 1.8.7.5.3 1 .5 1.6.5s1.1-.2 1.6-.5c.5-.2 1.1-.5 1.8-.7.8-.2 1.5-.3 2-.6.5-.3.8-.7.8-1.3 0-.5-.2-.9-.5-1.2.2-.4.3-.8.3-1.3 0-1.8-1-3.2-1.8-4.4-.7-1.1-1.3-1.8-1.3-2.6 0-.8.1-1.6.1-2.6C15 3.5 13.7 2 12 2zm-1.4 4.1c.4 0 .7.4.7.9s-.3.9-.7.9-.7-.4-.7-.9.3-.9.7-.9zm2.8 0c.4 0 .7.4.7.9s-.3.9-.7.9-.7-.4-.7-.9.3-.9.7-.9zM12 9.2c.7 0 1.9.5 1.9.9 0 .2-.6.5-1.1.7-.3.2-.6.3-.8.3s-.5-.1-.8-.3c-.5-.2-1.1-.5-1.1-.7 0-.4 1.2-.9 1.9-.9z" />
    </svg>
  );
}
