import Link from "next/link";
import { ArrowDown } from "lucide-react";

export function DownloadButton({
  size = "md",
  variant = "solid",
  className = "",
}: {
  size?: "sm" | "md" | "lg";
  variant?: "solid" | "outline";
  className?: string;
}) {
  return (
    <Link
      href="/download"
      className={`button ${size === "sm" ? "button-small" : ""} ${variant === "outline" ? "button-secondary" : ""} ${className}`}
    >
      Download Dragent <ArrowDown size={15} aria-hidden="true" />
    </Link>
  );
}
export function DownloadNote() {
  return (
    <p className="download-note">
      macOS & Windows <span aria-hidden="true">·</span> Free to get started
    </p>
  );
}
