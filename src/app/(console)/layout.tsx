import { notFound } from "next/navigation";
import { AppShell } from "@/components/shell/AppShell";

export default function ConsoleLayout({ children }: { children: React.ReactNode }) {
  if (process.env.NODE_ENV === "production") notFound();
  return <AppShell>{children}</AppShell>;
}
