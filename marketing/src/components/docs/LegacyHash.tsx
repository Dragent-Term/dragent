"use client";
import { useEffect } from "react";
import { useRouter } from "next/navigation";
export function LegacyHash({ ids }: { ids: string[] }) {
  const router = useRouter();
  useEffect(() => {
    const resolve = () => {
      const id = window.location.hash.slice(1);
      if (ids.includes(id)) router.replace(`/docs/${id}`);
    };
    resolve();
    window.addEventListener("hashchange", resolve);
    return () => window.removeEventListener("hashchange", resolve);
  }, [ids, router]);
  return null;
}
