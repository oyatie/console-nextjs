import type { Metadata } from "next";
import Link from "next/link";

export const dynamic = "force-dynamic";

export const metadata: Metadata = {
  title: "장비 매매·렌탈 | Oyatie",
  description: "공개된 장비를 살펴보고 문의하세요.",
};

export default function StorefrontLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen bg-slate-50 text-slate-900">
      <header className="border-b border-slate-200 bg-white">
        <div className="mx-auto flex max-w-6xl items-center justify-between px-5 py-4">
          <Link href="/storefront" className="text-lg font-bold tracking-tight">OYATIE 장비</Link>
          <span className="text-sm text-slate-600">매매 · 렌탈 문의</span>
        </div>
      </header>
      <main className="mx-auto max-w-6xl px-5 py-10">{children}</main>
      <footer className="border-t border-slate-200 px-5 py-6 text-center text-xs text-slate-500">OYATIE</footer>
    </div>
  );
}
