import Link from "next/link";
import { notFound } from "next/navigation";
import { getListing, isSupportedImageType, StorefrontApiError } from "../api";
import { conditionLabel, kindLabel, listingTypeLabel, mediaSrc, priceLabel } from "../display";
import { InquiryForm } from "../InquiryForm";

export default async function ListingPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  let listing;
  try {
    listing = await getListing(id);
  } catch (error) {
    if (error instanceof StorefrontApiError && error.status === 404) notFound();
    throw error;
  }

  const photos = listing.media
    .filter((media) => isSupportedImageType(media.content_type))
    .toSorted((a, b) => a.sort_order - b.sort_order);

  return (
    <div className="space-y-8">
      <Link href="/storefront" className="text-sm font-semibold text-amber-800 underline underline-offset-4">← 장비 목록</Link>
      <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_minmax(320px,400px)]">
        <article className="space-y-5 rounded-2xl border border-slate-200 bg-white p-6 shadow-sm">
          <div className="flex flex-wrap gap-2 text-sm text-slate-600">
            <span>{conditionLabel[listing.condition]}</span>
            <span aria-hidden="true">·</span>
            <span>{kindLabel[listing.kind]}</span>
            <span aria-hidden="true">·</span>
            <span>{listingTypeLabel[listing.listing_type]}</span>
          </div>
          <h1 className="text-3xl font-bold tracking-tight">{listing.model_name}</h1>
          {photos.length > 0 && (
            <div className="grid gap-3 sm:grid-cols-2">
              {photos.map((media, index) => (
                <img
                  key={media.id}
                  src={mediaSrc(listing.id, media.id)}
                  alt={media.alt_text?.trim() || `${listing.model_name} 사진 ${index + 1}`}
                  width={640}
                  height={400}
                  loading={index === 0 ? "eager" : "lazy"}
                  className="h-56 w-full rounded-xl object-cover"
                />
              ))}
            </div>
          )}
          {listing.status === "RESERVED" && <p className="text-sm font-semibold text-amber-800">현재 상담중인 장비입니다.</p>}
          <p className="text-2xl font-bold">{priceLabel(listing.price_won)}</p>
          <dl className="grid grid-cols-2 gap-4 border-t border-slate-200 pt-5 text-sm">
            {listing.capacity_milli !== null && <><dt className="text-slate-500">적재 중량</dt><dd>{listing.capacity_milli / 1000}톤</dd></>}
            {listing.model_year !== null && <><dt className="text-slate-500">연식</dt><dd>{listing.model_year}년</dd></>}
            {listing.usage_hours !== null && <><dt className="text-slate-500">사용 시간</dt><dd>{new Intl.NumberFormat("ko-KR").format(listing.usage_hours)}시간</dd></>}
            {listing.location && <><dt className="text-slate-500">위치</dt><dd>{listing.location}</dd></>}
            {listing.availability && <><dt className="text-slate-500">이용 가능 여부</dt><dd>{listing.availability}</dd></>}
          </dl>
          {listing.description && <p className="whitespace-pre-wrap border-t border-slate-200 pt-5 text-sm leading-7 text-slate-700">{listing.description}</p>}
        </article>
        <InquiryForm
          listingId={listing.id}
          defaultTopic={listing.listing_type === "RENTAL" ? "RENTAL" : listing.condition === "USED" ? "USED_SALES" : "OTHER"}
        />
      </div>
    </div>
  );
}
