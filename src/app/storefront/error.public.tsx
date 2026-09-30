"use client";

export default function StorefrontError({ retry }: { error: Error; retry: () => void }) {
  return (
    <div role="alert" className="rounded-2xl border border-red-200 bg-white p-8">
      <h1 className="text-xl font-semibold">장비 정보를 불러올 수 없습니다</h1>
      <p className="mt-2 text-sm text-slate-600">잠시 후 다시 시도해 주세요.</p>
      <button onClick={retry} className="mt-5 rounded-lg bg-slate-900 px-4 py-2 text-sm font-semibold text-white">다시 시도</button>
    </div>
  );
}
