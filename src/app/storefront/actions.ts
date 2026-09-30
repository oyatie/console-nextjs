"use server";

import { z } from "zod";
import { StorefrontApiError, submitInquiry } from "./api";

export type InquiryState = { status: "idle" | "pending" | "conflict" | "error"; message: string };

const inquirySchema = z.object({
  name: z.string().trim().min(1).max(100),
  phone: z.string().trim().min(1).max(40),
  topic: z.enum(["RENTAL", "USED_SALES", "MAINTENANCE", "OTHER"]),
  location: z.string().trim().max(120).optional(),
  message: z.string().trim().max(2000).optional(),
  listing_id: z.string().uuid().optional(),
});

export async function submitInquiryAction(
  _previous: InquiryState,
  formData: FormData,
): Promise<InquiryState> {
  const parsed = inquirySchema.safeParse({
    name: formData.get("name"),
    phone: formData.get("phone"),
    topic: formData.get("topic"),
    location: formData.get("location") || undefined,
    message: formData.get("message") || undefined,
    listing_id: formData.get("listing_id") || undefined,
  });
  if (!parsed.success) {
    return { status: "error", message: "입력한 내용을 확인해 주세요." };
  }

  try {
    await submitInquiry(parsed.data);
    // Rust's 202 follows a local commit, but has no two-site receipt or status lookup.
    return { status: "pending", message: "서버가 요청을 기록했습니다. 최종 접수 확인은 아직 할 수 없으니 중복 전송을 피해주세요." };
  } catch (error) {
    // ponytail: no inquiry idempotency key exists; a lost response may have committed. Add a receipt/status lookup before offering safe retries.
    if (error instanceof StorefrontApiError && error.status === 409 && parsed.data.listing_id) {
      return { status: "conflict", message: "선택한 장비가 변경되었거나 더 이상 공개되지 않아 문의가 접수되지 않았습니다. 내용을 확인하고 일반 문의로 전환할 수 있습니다." };
    }
    const message = error instanceof StorefrontApiError && error.status === 429
      ? "요청이 많습니다. 잠시 후 다시 시도해 주세요."
      : error instanceof StorefrontApiError && error.status === 400
        ? "입력한 내용을 확인해 주세요."
        : "접수 결과를 확인할 수 없습니다. 중복 문의를 피하려면 확인 후 다시 시도해 주세요.";
    return { status: "error", message };
  }
}
