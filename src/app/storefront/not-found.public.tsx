import Link from "next/link";

export default function StorefrontNotFound() {
  return (
    <div className="rounded-2xl border border-slate-200 bg-white p-8">
      <h1 className="text-xl font-semibold">장비를 찾을 수 없습니다</h1>
      <p className="mt-2 text-sm text-slate-600">공개가 종료되었거나 주소가 올바르지 않습니다.</p>
      <Link href="/storefront" className="mt-5 inline-block text-sm font-semibold underline">장비 목록으로</Link>
    </div>
  );
}
