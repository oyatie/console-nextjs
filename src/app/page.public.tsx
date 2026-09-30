import { redirect } from "next/navigation";

export default function RootPage() {
  redirect(process.env.NODE_ENV === "production" ? "/storefront" : "/dashboard");
}
