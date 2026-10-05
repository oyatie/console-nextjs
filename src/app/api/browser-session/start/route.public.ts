import { browserAction } from "@/lib/server/browser-session";
export const dynamic = "force-dynamic";
export function POST(request: Request) { return browserAction("start", request); }
