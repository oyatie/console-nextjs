import { getListingMedia, StorefrontApiError } from "../../../api";

export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ id: string; mediaId: string }> },
): Promise<Response> {
  const { id, mediaId } = await params;
  try {
    return await getListingMedia(id, mediaId);
  } catch (error) {
    const status = error instanceof StorefrontApiError && [404, 503].includes(error.status)
      ? error.status
      : 502;
    return new Response(status === 404 ? "Image not found" : "Image unavailable", {
      status,
      headers: { "Cache-Control": "no-store", "Content-Type": "text/plain; charset=utf-8" },
    });
  }
}
