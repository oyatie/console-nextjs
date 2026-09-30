import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { getCatalog, getListing, submitInquiry } from "@/app/storefront/api";
import { submitInquiryAction } from "@/app/storefront/actions";
import { GET as getMedia } from "@/app/storefront/[id]/media/[mediaId]/route.public";
import StorefrontPage from "@/app/storefront/page.public";

const listing = {
  id: "00000000-0000-4000-8000-000000000001",
  model_name: "전동 지게차",
  kind: "ELECTRIC",
  condition: "USED",
  listing_type: "SALE",
  status: "PUBLISHED",
  capacity_milli: 2500,
  model_year: 2022,
  usage_hours: 100,
  price_won: 10000000,
  availability: "판매 가능",
  location: "서울",
  description: "실제 공개 장비",
  media: [],
};

const mediaId = "00000000-0000-4000-8000-000000000002";
const listingWithMedia = {
  ...listing,
  media: [{
    id: mediaId,
    url: `/api/v1/storefront/listings/${listing.id}/media/${mediaId}`,
    content_type: "image/png",
    alt_text: "지게차 측면",
    sort_order: 0,
  }],
};

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("public storefront boundary", () => {
  it("requires an explicit server backend origin", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "");
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    await expect(getCatalog({ offset: 0 })).rejects.toMatchObject({ status: 503 });
    expect(fetch).not.toHaveBeenCalled();
  });

  it("reads the live paged catalog without caching or accepting a redirect", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [listing], total: 1, limit: 24, offset: 0 }), { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    const page = await getCatalog({ condition: "USED", offset: 0 });
    expect(page.items[0].model_name).toBe("전동 지게차");
    const [url, options] = fetch.mock.calls[0] as [URL, RequestInit];
    expect(url.toString()).toBe("http://127.0.0.1:8080/api/v1/storefront/listings?limit=24&offset=0&condition=USED");
    expect(options).toMatchObject({ cache: "no-store", redirect: "error" });
  });

  it("fails closed on invalid origin, path ID, or malformed API data", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "https://user:secret@example.com");
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    await expect(getCatalog({ offset: 0 })).rejects.toMatchObject({ status: 503 });
    await expect(getListing("../admin")).rejects.toMatchObject({ status: 404 });
    expect(fetch).not.toHaveBeenCalled();

    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    fetch.mockResolvedValueOnce(new Response(JSON.stringify({ items: [{ ...listing, status: "DRAFT" }], total: 1, limit: 24, offset: 0 }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ items: [{ ...listing, price_won: Number.MAX_SAFE_INTEGER + 1 }], total: 1, limit: 24, offset: 0 }), { status: 200 }));
    await expect(getCatalog({ offset: 0 })).rejects.toMatchObject({ status: 502 });
    await expect(getCatalog({ offset: 0 })).rejects.toMatchObject({ status: 502 });
  });

  it("accepts only Rust's fixed media URL in listing metadata", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify(listingWithMedia), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ...listingWithMedia, media: [{ ...listingWithMedia.media[0], url: "https://example.com/private" }] }), { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    expect((await getListing(listing.id)).media[0].alt_text).toBe("지게차 측면");
    await expect(getListing(listing.id)).rejects.toMatchObject({ status: 502 });
  });

  it("streams only safe raster media from the fixed Rust path with no caching", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn().mockResolvedValue(new Response(Uint8Array.from([137, 80, 78, 71]), {
      status: 200,
      headers: { "Content-Type": "image/png" },
    }));
    vi.stubGlobal("fetch", fetch);
    const response = await getMedia(new Request("http://localhost/storefront"), { params: Promise.resolve({ id: listing.id, mediaId }) });
    expect(response.status).toBe(200);
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    expect(response.headers.get("X-Content-Type-Options")).toBe("nosniff");
    expect(Array.from(new Uint8Array(await response.arrayBuffer()))).toEqual([137, 80, 78, 71]);
    const [url, options] = fetch.mock.calls[0] as [URL, RequestInit];
    expect(url.toString()).toBe(`http://127.0.0.1:8080/api/v1/storefront/listings/${listing.id}/media/${mediaId}`);
    expect(options).toMatchObject({ cache: "no-store", redirect: "error" });
  });

  it("keeps an accepted media body alive after the response-header deadline", async () => {
    vi.useFakeTimers();
    vi.spyOn(AbortSignal, "timeout").mockImplementation((milliseconds) => {
      const controller = new AbortController();
      setTimeout(() => controller.abort(), milliseconds);
      return controller.signal;
    });
    let upstreamSignal: AbortSignal | undefined;
    vi.stubGlobal("fetch", vi.fn().mockImplementation((_url: URL, options: RequestInit) => {
      upstreamSignal = options.signal as AbortSignal;
      return Promise.resolve(new Response(Uint8Array.from([137, 80, 78, 71]), {
        status: 200,
        headers: { "Content-Type": "image/png" },
      }));
    }));
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");

    const response = await getMedia(new Request("http://localhost/storefront"), {
      params: Promise.resolve({ id: listing.id, mediaId }),
    });
    await vi.advanceTimersByTimeAsync(5001);
    expect(upstreamSignal?.aborted).toBe(false);
    expect(Array.from(new Uint8Array(await response.arrayBuffer()))).toEqual([137, 80, 78, 71]);
  });

  it("allows a slow media body and clears its deadline after completion", async () => {
    vi.useFakeTimers();
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    let source: ReadableStreamDefaultController<Uint8Array> | undefined;
    let upstreamSignal: AbortSignal | undefined;
    vi.stubGlobal("fetch", vi.fn().mockImplementation((_url: URL, options: RequestInit) => {
      upstreamSignal = options.signal as AbortSignal;
      return Promise.resolve(new Response(new ReadableStream<Uint8Array>({
        start(controller) { source = controller; },
      }), { headers: { "Content-Type": "image/png" } }));
    }));

    const response = await getMedia(new Request("http://localhost/storefront"), {
      params: Promise.resolve({ id: listing.id, mediaId }),
    });
    const body = response.arrayBuffer();
    await vi.advanceTimersByTimeAsync(10_000);
    source?.enqueue(Uint8Array.from([137, 80, 78, 71]));
    source?.close();
    expect(Array.from(new Uint8Array(await body))).toEqual([137, 80, 78, 71]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(upstreamSignal?.aborted).toBe(false);
  });

  it("aborts a stalled media body and releases its timer when canceled", async () => {
    vi.useFakeTimers();
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    let upstreamSignal: AbortSignal | undefined;
    let canceled = 0;
    vi.stubGlobal("fetch", vi.fn().mockImplementation((_url: URL, options: RequestInit) => {
      upstreamSignal = options.signal as AbortSignal;
      return Promise.resolve(new Response(new ReadableStream<Uint8Array>({
        cancel() { canceled += 1; },
      }), { headers: { "Content-Type": "image/png" } }));
    }));
    const context = { params: Promise.resolve({ id: listing.id, mediaId }) };

    const stalled = await getMedia(new Request("http://localhost/storefront"), context);
    const failure = expect(stalled.arrayBuffer()).rejects.toThrow();
    await vi.advanceTimersByTimeAsync(60_001);
    await failure;
    expect(upstreamSignal?.aborted).toBe(true);
    expect(canceled).toBe(1);

    const disconnected = await getMedia(new Request("http://localhost/storefront"), context);
    await disconnected.body?.cancel();
    await vi.advanceTimersByTimeAsync(60_001);
    expect(upstreamSignal?.aborted).toBe(false);
    expect(canceled).toBe(2);
  });

  it("returns 404 for invalid or withdrawn media and rejects unsafe MIME", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn().mockResolvedValueOnce(new Response("", { status: 404 }))
      .mockResolvedValueOnce(new Response("<svg />", { status: 200, headers: { "Content-Type": "image/svg+xml" } }));
    vi.stubGlobal("fetch", fetch);
    const context = (id: string, mid: string) => ({ params: Promise.resolve({ id, mediaId: mid }) });
    expect((await getMedia(new Request("http://localhost/storefront"), context("../admin", mediaId))).status).toBe(404);
    expect(fetch).not.toHaveBeenCalled();
    expect((await getMedia(new Request("http://localhost/storefront"), context(listing.id, mediaId))).status).toBe(404);
    const unsafe = await getMedia(new Request("http://localhost/storefront"), context(listing.id, mediaId));
    expect(unsafe.status).toBe(502);
    expect(unsafe.headers.get("Cache-Control")).toBe("no-store");
  });

  it("accepts only Rust's exact inquiry response shape", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({ status: "received" }), { status: 202 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ status: "received" }), { status: 500 }));
    vi.stubGlobal("fetch", fetch);
    const inquiry = { name: "홍길동", phone: "010-1234-5678", topic: "USED_SALES" as const, listing_id: listing.id };
    await expect(submitInquiry(inquiry)).resolves.toBeUndefined();
    const [, options] = fetch.mock.calls[0] as [URL, RequestInit];
    expect(JSON.parse(options.body as string)).toEqual(inquiry);
    await expect(submitInquiry(inquiry)).rejects.toMatchObject({ status: 500 });
  });

  it("keeps a general inquiry pending when the storefront catalog is empty", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ items: [], total: 0, limit: 24, offset: 0 }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ status: "received" }), { status: 202 }));
    vi.stubGlobal("fetch", fetch);

    const html = renderToStaticMarkup(await StorefrontPage({ searchParams: Promise.resolve({}) }));
    expect(html).toContain("일반 문의하기");
    expect(html).toContain('name="phone"');
    expect(html).not.toContain('name="listing_id"');

    const form = new FormData();
    form.set("name", "홍길동");
    form.set("phone", "010-1234-5678");
    form.set("topic", "OTHER");
    expect(await submitInquiryAction({ status: "idle", message: "" }, form)).toMatchObject({
      status: "pending",
      message: expect.stringContaining("최종 접수 확인은 아직 할 수 없으니"),
    });
    const [, options] = fetch.mock.calls[1] as [URL, RequestInit];
    expect(JSON.parse(options.body as string)).toEqual({ name: "홍길동", phone: "010-1234-5678", topic: "OTHER" });
  });

  it("rejects malformed form data before calling Rust", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const form = new FormData();
    form.set("name", "");
    form.set("phone", "010-1234-5678");
    form.set("topic", "USED_SALES");
    form.set("listing_id", listing.id);
    const result = await submitInquiryAction({ status: "idle", message: "" }, form);
    expect(result.status).toBe("error");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("does not claim success when an inquiry response is uncertain", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("", { status: 500 })));
    const form = new FormData();
    form.set("name", "홍길동");
    form.set("phone", "010-1234-5678");
    form.set("topic", "USED_SALES");
    form.set("listing_id", listing.id);
    const result = await submitInquiryAction({ status: "idle", message: "" }, form);
    expect(result).toMatchObject({ status: "error" });
    expect(result.message).toContain("접수 결과를 확인할 수 없습니다");
  });

  it("reports a changed listing as a conflict without exposing backend detail", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ detail: "private row data" }), { status: 409 })));
    const form = new FormData();
    form.set("name", "홍길동");
    form.set("phone", "010-1234-5678");
    form.set("topic", "USED_SALES");
    form.set("listing_id", listing.id);
    const result = await submitInquiryAction({ status: "idle", message: "" }, form);
    expect(result).toMatchObject({ status: "conflict", message: expect.stringContaining("장비") });
    expect(result.message).not.toContain("private row data");
  });

  it("shows a validation error for a rejected inquiry", async () => {
    vi.stubEnv("CONSOLE_BACKEND_ORIGIN", "http://127.0.0.1:8080");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("", { status: 400 })));
    const form = new FormData();
    form.set("name", "홍길동");
    form.set("phone", "010-1234-5678");
    form.set("topic", "OTHER");
    expect(await submitInquiryAction({ status: "idle", message: "" }, form)).toMatchObject({
      status: "error",
      message: "입력한 내용을 확인해 주세요.",
    });
  });
});
