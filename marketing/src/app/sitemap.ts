import type { MetadataRoute } from "next";
import { allDocs } from "@/components/docs/catalog";
export default function sitemap(): MetadataRoute.Sitemap {
  return [
    "",
    "/docs",
    "/pricing",
    "/download",
    "/privacy",
    "/terms",
    "/refund",
    "/licenses",
    ...allDocs.map((doc) => `/docs/${doc.id}`),
  ].map((path) => ({
    url: `https://dragent.dev${path}`,
    changeFrequency: path.startsWith("/docs") ? "weekly" : "monthly",
    priority: path === "" ? 1 : path.startsWith("/docs/") ? 0.7 : 0.8,
  }));
}
