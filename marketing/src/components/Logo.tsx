import Image from "next/image";

/**
 * The standalone Dragent ribbon mark. Marketing surfaces use the transparent
 * mark; the boxed app icon remains available for favicons and native apps.
 */
export function Mark({ size = 30 }: { size?: number }) {
  return (
    <Image
      src="/dragent-mark.png"
      alt=""
      width={size}
      height={size}
      priority
      className="shrink-0 object-contain"
      aria-hidden="true"
    />
  );
}

export function Logo({ size = 30 }: { size?: number }) {
  return (
    <span className="inline-flex items-center gap-2.5 select-none">
      <Mark size={size} />
      <span
        className="font-[family-name:var(--font-outfit)] font-semibold tracking-tight text-[1.15rem] text-theme-fg"
        style={{ letterSpacing: "-0.02em" }}
      >
        Dragent
      </span>
    </span>
  );
}
