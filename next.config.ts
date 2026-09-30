import type { NextConfig } from "next";
import path from "path";

const nextConfig: NextConfig = {
  output: "standalone",
  distDir: process.env.NEXT_E2E_DIST_DIR === "1" ? ".next-e2e" : ".next",
  trailingSlash: true,
  allowedDevOrigins: ["127.0.0.1"],
  pageExtensions: process.env.NODE_ENV === "production"
    ? ["public.tsx", "public.ts"]
    : ["public.tsx", "public.ts", "tsx", "ts"],
  outputFileTracingRoot: path.resolve(__dirname),
  images: {
    unoptimized: true,
  },
  typescript: {
    ignoreBuildErrors: false,
  },
};

export default nextConfig;
