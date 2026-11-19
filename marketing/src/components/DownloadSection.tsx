import Link from "next/link";
import { ArrowDown, ArrowUpRight } from "lucide-react";
import { AppleLogo, WindowsLogo, LinuxLogo } from "./BrandIcons";
import { site } from "@/lib/site";

const platforms = [
  {
    name: "macOS",
    icon: AppleLogo,
    note: "Open the disk image and drag Dragent into Applications.",
    builds: [
      {
        name: "Apple Silicon",
        detail: "M-series Macs · arm64 · DMG",
        href: site.downloads.mac.arm64,
      },
      {
        name: "Intel",
        detail: "Intel-based Macs · x86_64 · DMG",
        href: site.downloads.mac.x64,
      },
    ],
  },
  {
    name: "Windows",
    icon: WindowsLogo,
    note: "Run the installer, then open Dragent from the Start menu.",
    builds: [
      {
        name: "Windows x64",
        detail: "Intel & AMD · 64-bit · EXE",
        href: site.downloads.windows.x64,
      },
      {
        name: "Windows Arm64",
        detail: "ARM-based PCs · EXE",
        href: site.downloads.windows.arm64,
      },
    ],
  },
];
export function DownloadSection() {
  return (
    <section
      id="download"
      className="downloads-section container-wide"
      aria-label="Dragent desktop installers"
    >
      <div className="download-builds">
        {platforms.map(({ name, icon: Icon, note, builds }) => (
          <article className="download-platform" key={name}>
            <Icon size={30} />
            <h2>{name}</h2>
            <p>{note}</p>
            <div className="download-build-links">
              {builds.map((build) => (
                <a
                  href={build.href}
                  key={build.name}
                  aria-label={`Download Dragent for ${name}, ${build.name}`}
                >
                  <span>
                    <strong>{build.name}</strong>
                    <small>{build.detail}</small>
                  </span>
                  <ArrowDown size={17} aria-hidden="true" />
                </a>
              ))}
            </div>
          </article>
        ))}
        <article className="download-platform linux-platform">
          <LinuxLogo size={30} />
          <h2>Linux</h2>
          <p>
            A Linux desktop installer is not currently available. You can
            connect supported Linux machines as SSH hosts.
          </p>
          <Link className="text-link" href="/docs/remote-ssh">
            Use a remote Linux host <ArrowUpRight size={14} />
          </Link>
          <div className="linux-note">
            macOS and glibc-based Linux hosts are supported. See the remote
            guide for prerequisites.
          </div>
        </article>
      </div>
      <div className="download-help">
        <div>
          <h3>Which build do I need?</h3>
          <p>
            On a Mac, check Apple menu → About This Mac for the chip or
            processor. On Windows, check Settings → System → About → System
            type.
          </p>
        </div>
        <div>
          <h3>Ready for your first task?</h3>
          <p>
            Install Git and authenticate your preferred agent CLI. Your Dragent
            account and provider login are separate.
          </p>
          <Link href="/docs/quickstart">
            Follow the quickstart <ArrowRightIcon />
          </Link>
        </div>
      </div>
    </section>
  );
}
function ArrowRightIcon() {
  return <ArrowUpRight size={13} aria-hidden="true" />;
}
