import type { Listing } from "./api";

export const kindLabel: Record<Listing["kind"], string> = {
  ELECTRIC: "전동",
  DIESEL: "디젤",
  LPG: "LPG",
  REACH: "리치",
};

export const conditionLabel: Record<Listing["condition"], string> = {
  USED: "중고",
  NEW: "신차",
};

export const listingTypeLabel: Record<Listing["listing_type"], string> = {
  SALE: "매매",
  RENTAL: "렌탈",
  BOTH: "매매 · 렌탈",
};

export function priceLabel(price: number | null): string {
  return price === null ? "가격 문의" : `${new Intl.NumberFormat("ko-KR").format(price)}원`;
}

export function mediaSrc(listingId: string, mediaId: string): string {
  return `/storefront/${listingId}/media/${mediaId}/`;
}
