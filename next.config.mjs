import path from "node:path";
import { fileURLToPath } from "node:url";

/** @type {import('next').NextConfig} */
const nextConfig = {
  distDir: process.env.NEXT_E2E_DIST_DIR === "1" ? ".next-e2e" : ".next",
  trailingSlash: true,
  allowedDevOrigins: ["127.0.0.1"],
  pageExtensions: process.env.NODE_ENV === "production"
    ? ["public.tsx", "public.ts"]
    : ["public.tsx", "public.ts", "tsx", "ts"],
  outputFileTracingRoot: path.dirname(fileURLToPath(import.meta.url)),
  images: { unoptimized: true },
  typescript: { ignoreBuildErrors: false },
};
export default nextConfig;
