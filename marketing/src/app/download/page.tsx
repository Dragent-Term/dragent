import type { Metadata } from "next";
import Link from "next/link";
import { ArrowUpRight } from "lucide-react";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { DownloadSection } from "@/components/DownloadSection";
export const metadata: Metadata = {
  title: "Download",
  description:
    "Download Dragent for macOS or Windows. Choose Apple Silicon, Intel, x64, or Arm64 and start your first agent workspace.",
  alternates: { canonical: "/download" },
};
export default function DownloadPage() {
  return (
    <>
      <Nav />
      <main id="main-content" className="download-page">
        <header className="download-heading container-wide">
          <p className="eyebrow">Your next workspace</p>
          <h1>
            Bring your agents.
            <br />
            We’ll make room.
          </h1>
          <p>
            Get Dragent for your machine. Start free, with the tools and agents
            you already use.
          </p>
          <Link href="/docs/install">
            Installation guide <ArrowUpRight size={14} />
          </Link>
        </header>
        <DownloadSection />
        <div className="download-bottom container-wide">
          <span>Looking for help with installation?</span>
          <Link href="/docs/troubleshooting">
            Troubleshooting <ArrowUpRight size={13} />
          </Link>
        </div>
      </main>
      <Footer />
    </>
  );
}
