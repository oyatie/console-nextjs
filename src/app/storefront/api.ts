import { z } from "zod";
import { backendRequest } from "@/lib/server/backend";

const mediaSchema = z.object({
  id: z.string().uuid(),
  url: z.string(),
  content_type: z.string().min(1),
  alt_text: z.string().nullable(),
  sort_order: z.number().int(),
});

const listingSchema = z.object({
  id: z.string().uuid(),
  model_name: z.string().min(1),
  kind: z.enum(["ELECTRIC", "DIESEL", "LPG", "REACH"]),
  condition: z.enum(["USED", "NEW"]),
  listing_type: z.enum(["SALE", "RENTAL", "BOTH"]),
  status: z.enum(["PUBLISHED", "RESERVED"]),
  capacity_milli: z.number().int().nonnegative().safe().nullable(),
  model_year: z.number().int().nullable(),
  usage_hours: z.number().int().nonnegative().nullable(),
  price_won: z.number().int().nonnegative().safe().nullable(),
  availability: z.string().nullable(),
  location: z.string().nullable(),
  description: z.string().nullable(),
  media: z.array(mediaSchema),
}).refine(
  (listing) => listing.media.every((media) =>
    media.url === `/api/v1/storefront/listings/${listing.id}/media/${media.id}`),
  "Invalid listing media path",
);

const catalogSchema = z.object({
  items: z.array(listingSchema),
  total: z.number().int().nonnegative().safe(),
  limit: z.number().int().positive().safe(),
  offset: z.number().int().nonnegative().safe(),
});

export type Listing = z.infer<typeof listingSchema>;
export type Catalog = z.infer<typeof catalogSchema>;
export type CatalogFilters = {
  kind?: Listing["kind"];
  condition?: Listing["condition"];
  listing_type?: Listing["listing_type"];
  offset: number;
};

const imageTypes = new Set(["image/jpeg", "image/png", "image/webp", "image/avif", "image/gif"]);

export function isSupportedImageType(value: string): boolean {
  return imageTypes.has(value.split(";", 1)[0].trim().toLowerCase());
}

export class StorefrontApiError extends Error {
  constructor(public readonly status: number) {
    super("Storefront API unavailable");
  }
}

async function request(path: string, options?: RequestInit): Promise<Response> {
  try {
    return await backendRequest(path, options);
  } catch (error) {
    if (error instanceof StorefrontApiError) throw error;
    throw new StorefrontApiError(503);
  }
}

async function read<T>(path: string, schema: z.ZodType<T>): Promise<T> {
  const response = await request(path);
  if (!response.ok) throw new StorefrontApiError(response.status);
  try {
    return schema.parse(await response.json());
  } catch {
    throw new StorefrontApiError(502);
  }
}

export async function getCatalog(filters: CatalogFilters): Promise<Catalog> {
  const query = new URLSearchParams({ limit: "24", offset: String(filters.offset) });
  if (filters.kind) query.set("kind", filters.kind);
  if (filters.condition) query.set("condition", filters.condition);
  if (filters.listing_type) query.set("listing_type", filters.listing_type);
  return read(`/api/v1/storefront/listings?${query}`, catalogSchema);
}

export async function getListing(id: string): Promise<Listing> {
  if (!z.string().uuid().safeParse(id).success) throw new StorefrontApiError(404);
  return read(`/api/v1/storefront/listings/${id}`, listingSchema);
}

export async function getListingMedia(listingId: string, mediaId: string): Promise<Response> {
  if (!z.string().uuid().safeParse(listingId).success || !z.string().uuid().safeParse(mediaId).success) {
    throw new StorefrontApiError(404);
  }
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);
  let response: Response;
  try {
    response = await request(`/api/v1/storefront/listings/${listingId}/media/${mediaId}`, {
      signal: controller.signal,
    });
  } finally {
    clearTimeout(timeout);
  }
  if (response.status !== 200) {
    controller.abort();
    throw new StorefrontApiError(response.status);
  }
  const contentType = response.headers.get("content-type");
  if (!contentType || !isSupportedImageType(contentType) || !response.body) {
    controller.abort();
    throw new StorefrontApiError(502);
  }
  const transferTimeout = setTimeout(() => controller.abort(), 60_000);
  const passthrough = new TransformStream<Uint8Array, Uint8Array>();
  void response.body.pipeTo(passthrough.writable, { signal: controller.signal }).then(
    () => clearTimeout(transferTimeout),
    () => clearTimeout(transferTimeout),
  );
  return new Response(passthrough.readable, {
    headers: {
      "Content-Type": contentType.split(";", 1)[0].trim().toLowerCase(),
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

export type Inquiry = {
  name: string;
  phone: string;
  topic: "RENTAL" | "USED_SALES" | "MAINTENANCE" | "OTHER";
  location?: string;
  message?: string;
  listing_id?: string;
};

export async function submitInquiry(inquiry: Inquiry): Promise<void> {
  const response = await request("/api/v1/storefront/inquiries", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(inquiry),
  });
  if (response.status !== 202) throw new StorefrontApiError(response.status);
  try {
    z.object({ status: z.literal("received") }).parse(await response.json());
  } catch {
    throw new StorefrontApiError(502);
  }
}
