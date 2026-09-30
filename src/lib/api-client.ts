/** A request is successful only when the live API returns a usable response. */
export const API_CONFIG = {
  baseUrl: process.env.NEXT_PUBLIC_API_URL || "/api",
  timeoutMs: 5000,
};

export interface BackendHealthStatus {
  connected: boolean;
  latencyMs: number;
  apiBase: string;
  mode: "LIVE_GATEWAY" | "UNAVAILABLE";
}

export async function checkBackendHealth(): Promise<BackendHealthStatus> {
  const started = Date.now();
  let connected = false;
  try {
    const response = await fetch(`${API_CONFIG.baseUrl}/healthz`, {
      cache: "no-store",
      signal: AbortSignal.timeout(2000),
    });
    connected = response.ok;
  } catch {
    connected = false;
  }
  return {
    connected,
    latencyMs: Date.now() - started,
    apiBase: API_CONFIG.baseUrl,
    mode: connected ? "LIVE_GATEWAY" : "UNAVAILABLE",
  };
}

export type ApiResult<T> =
  | { success: true; data: T; status: number }
  | { success: false; error: string; status?: number };

export async function apiRequest<T>(endpoint: string, options: RequestInit = {}): Promise<ApiResult<T>> {
  const url = `${API_CONFIG.baseUrl}${endpoint.startsWith("/") ? endpoint : `/${endpoint}`}`;
  try {
    const headers = new Headers(options.headers);
    if (!headers.has("Accept")) headers.set("Accept", "application/json");
    const response = await fetch(url, {
      ...options,
      cache: "no-store",
      signal: options.signal
        ? AbortSignal.any([options.signal, AbortSignal.timeout(API_CONFIG.timeoutMs)])
        : AbortSignal.timeout(API_CONFIG.timeoutMs),
      headers,
    });
    if (!response.ok) {
      return { success: false, status: response.status, error: `HTTP ${response.status}` };
    }
    if (response.status === 204) {
      return { success: true, status: 204, data: undefined as T };
    }
    try {
      return { success: true, status: response.status, data: (await response.json()) as T };
    } catch {
      return { success: false, status: response.status, error: "Invalid API response" };
    }
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : "API unavailable" };
  }
}
