import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { DocsShell } from "@/components/docs/DocsShell";
import { allDocs, getDoc } from "@/components/docs/catalog";
export const dynamicParams = false;
export function generateStaticParams() {
  return allDocs.map((doc) => ({ slug: doc.id }));
}
export async function generateMetadata({
  params,
}: {
  params: Promise<{ slug: string }>;
}): Promise<Metadata> {
  const { slug } = await params;
  const doc = getDoc(slug);
  return {
    title: doc ? `${doc.title} — Docs` : "Page not found",
    description:
      doc?.summary ??
      (doc ? `Learn about ${doc.label.toLowerCase()} in Dragent.` : undefined),
    alternates: { canonical: `/docs/${slug}` },
    openGraph: {
      title: doc?.title,
      description: doc?.summary,
      url: `/docs/${slug}`,
      type: "article",
      images: [{ url: "/og.png", width: 1200, height: 630 }],
    },
  };
}
export default async function DocPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  const doc = getDoc(slug);
  if (!doc) notFound();
  return <DocsShell current={doc} />;
}
