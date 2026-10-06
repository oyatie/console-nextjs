/** Shared server-only transport. It does not infer identity from request headers. */
export function backendUrl(path: string): URL {
  const configured = process.env.CONSOLE_BACKEND_ORIGIN?.trim();
  if (!configured) throw new Error("Backend unavailable");
  const origin = new URL(configured);
  if (!['http:', 'https:'].includes(origin.protocol) || origin.username || origin.password ||
      origin.pathname !== '/' || origin.search || origin.hash) throw new Error("Backend unavailable");
  return new URL(path, origin);
}

export async function backendRequest(path: string, options?: RequestInit): Promise<Response> {
  return fetch(backendUrl(path), {
    ...options, cache: "no-store", redirect: "error",
    signal: options?.signal ?? AbortSignal.timeout(5000),
  });
}

/** Bound bytes before decoding, including responses without Content-Length. */
export async function boundedBytes(body: ReadableStream<Uint8Array> | null, limit: number): Promise<Buffer> {
  if (!body) return Buffer.alloc(0);
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw new RangeError("Body exceeds admission");
      chunks.push(value);
    }
    return Buffer.concat(chunks, size);
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}
