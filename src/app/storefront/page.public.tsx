import Link from "next/link";
import { getCatalog, isSupportedImageType, type CatalogFilters, type Listing } from "./api";
import { conditionLabel, kindLabel, listingTypeLabel, mediaSrc, priceLabel } from "./display";
import { InquiryForm } from "./InquiryForm";

type Query = Record<string, string | string[] | undefined>;

function choice<T extends string>(value: Query[string], allowed: readonly T[]): T | undefined {
  return typeof value === "string" && allowed.includes(value as T) ? value as T : undefined;
}

function filtersFromQuery(query: Query): CatalogFilters {
  const rawOffset = query.offset;
  const offset = typeof rawOffset === "string" && /^\d+$/.test(rawOffset)
    ? Math.min(Number(rawOffset), 1_000_000)
    : 0;
  return {
    kind: choice<Listing["kind"]>(query.kind, ["ELECTRIC", "DIESEL", "LPG", "REACH"]),
    condition: choice<Listing["condition"]>(query.condition, ["USED", "NEW"]),
    listing_type: choice<Listing["listing_type"]>(query.listing_type, ["SALE", "RENTAL", "BOTH"]),
    offset: Number.isSafeInteger(offset) ? offset : 0,
  };
}

function pageHref(filters: CatalogFilters, offset: number): string {
  const query = new URLSearchParams();
  if (filters.kind) query.set("kind", filters.kind);
  if (filters.condition) query.set("condition", filters.condition);
  if (filters.listing_type) query.set("listing_type", filters.listing_type);
  if (offset > 0) query.set("offset", String(offset));
  return `/storefront${query.size ? `?${query}` : ""}`;
}

export default async function StorefrontPage({ searchParams }: { searchParams: Promise<Query> }) {
  const filters = filtersFromQuery(await searchParams);
  const catalog = await getCatalog(filters);

  return (
    <div className="space-y-8">
      <div>
        <p className="text-sm font-semibold text-amber-700">공개 장비 목록</p>
        <h1 className="mt-2 text-3xl font-bold tracking-tight">장비 매매·렌탈</h1>
        <p className="mt-3 text-slate-600">공개된 장비를 살펴보거나 아래에서 일반 문의를 남길 수 있습니다.</p>
      </div>

      <form method="get" className="grid gap-3 rounded-2xl border border-slate-200 bg-white p-5 sm:grid-cols-4">
        <label className="text-sm font-medium">
          동력
          <select name="kind" defaultValue={filters.kind ?? ""} className="mt-1 block w-full rounded-lg border border-slate-300 bg-white px-3 py-2">
            <option value="">전체</option>
            {Object.entries(kindLabel).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        <label className="text-sm font-medium">
          상태
          <select name="condition" defaultValue={filters.condition ?? ""} className="mt-1 block w-full rounded-lg border border-slate-300 bg-white px-3 py-2">
            <option value="">전체</option>
            {Object.entries(conditionLabel).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        <label className="text-sm font-medium">
          거래 방식
          <select name="listing_type" defaultValue={filters.listing_type ?? ""} className="mt-1 block w-full rounded-lg border border-slate-300 bg-white px-3 py-2">
            <option value="">전체</option>
            {Object.entries(listingTypeLabel).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        <button type="submit" className="self-end rounded-lg bg-slate-900 px-4 py-2 text-sm font-semibold text-white hover:bg-slate-700">찾기</button>
      </form>

      <section aria-label="장비 검색 결과" className="space-y-4">
        <p className="text-sm text-slate-600">검색 결과 {catalog.total}건</p>
        {catalog.items.length === 0 ? (
          <div className="rounded-2xl border border-slate-200 bg-white p-8 text-center text-slate-600">
            {filters.offset > 0 ? (
              <>
                <p>이 페이지에 표시할 장비가 없습니다.</p>
                <Link href={pageHref(filters, 0)} className="mt-3 inline-block font-semibold text-amber-800 underline">첫 페이지로</Link>
              </>
            ) : filters.kind || filters.condition || filters.listing_type ? (
              "조건에 맞는 공개 장비가 없습니다."
            ) : (
              "현재 공개된 장비가 없습니다."
            )}
          </div>
        ) : (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {catalog.items.map((listing) => (
              <article key={listing.id} className="flex flex-col rounded-2xl border border-slate-200 bg-white p-5 shadow-sm">
                {listing.media.filter((media) => isSupportedImageType(media.content_type)).slice(0, 1).map((media) => (
                  <img
                    key={media.id}
                    src={mediaSrc(listing.id, media.id)}
                    alt={media.alt_text?.trim() || `${listing.model_name} 장비 사진`}
                    width={640}
                    height={400}
                    loading="lazy"
                    className="mb-4 h-48 w-full rounded-xl object-cover"
                  />
                ))}
                <div className="flex items-center gap-2 text-xs font-medium text-slate-600">
                  <span>{conditionLabel[listing.condition]}</span>
                  <span aria-hidden="true">·</span>
                  <span>{kindLabel[listing.kind]}</span>
                  {listing.status === "RESERVED" && <span className="rounded-full bg-amber-100 px-2 py-0.5 text-amber-800">상담중</span>}
                </div>
                <h2 className="mt-3 text-lg font-semibold">{listing.model_name}</h2>
                <p className="mt-1 text-sm text-slate-600">{listingTypeLabel[listing.listing_type]}{listing.location ? ` · ${listing.location}` : ""}</p>
                <p className="mt-4 text-lg font-bold">{priceLabel(listing.price_won)}</p>
                <Link href={`/storefront/${listing.id}`} className="mt-5 inline-block self-start text-sm font-semibold text-amber-800 underline underline-offset-4">상세 보기</Link>
              </article>
            ))}
          </div>
        )}
        {catalog.total > catalog.limit && (
          <nav aria-label="목록 페이지" className="flex items-center justify-between pt-3 text-sm font-semibold">
            {catalog.offset > 0 ? <Link href={pageHref(filters, Math.max(0, catalog.offset - catalog.limit))}>이전</Link> : <span />}
            {catalog.offset + catalog.items.length < catalog.total && <Link href={pageHref(filters, catalog.offset + catalog.limit)}>다음</Link>}
          </nav>
        )}
      </section>
      <InquiryForm />
    </div>
  );
}
