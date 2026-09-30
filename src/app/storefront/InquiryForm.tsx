"use client";

import { useActionState, useEffect, useRef, useState } from "react";
import { submitInquiryAction, type InquiryState } from "./actions";

const initialState: InquiryState = { status: "idle", message: "" };

export function InquiryForm({ listingId, defaultTopic = "OTHER" }: {
  listingId?: string;
  defaultTopic?: "RENTAL" | "USED_SALES" | "OTHER";
}) {
  const [state, action, pending] = useActionState(submitInquiryAction, initialState);
  const [draft, setDraft] = useState({ name: "", phone: "", topic: defaultTopic as string, location: "", message: "" });
  const [generalInquiry, setGeneralInquiry] = useState(false);
  const activeListingId = generalInquiry ? undefined : listingId;
  const statusRef = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if ((generalInquiry && state.status === "conflict") || state.status === "pending") statusRef.current?.focus();
  }, [generalInquiry, state.status]);
  function update(field: keyof typeof draft, value: string) {
    setDraft((current) => ({ ...current, [field]: value }));
  }

  if (state.status === "pending") {
    return <p ref={statusRef} role="status" tabIndex={-1} className="rounded-xl border border-amber-200 bg-amber-50 p-5 text-amber-900 focus:outline-2 focus:outline-offset-2 focus:outline-amber-600">{state.message}</p>;
  }

  return (
    <form action={action} className="space-y-4 rounded-2xl border border-slate-200 bg-white p-5 shadow-sm">
      <h2 className="text-lg font-semibold text-slate-900">{activeListingId ? "이 장비 문의하기" : "일반 문의하기"}</h2>
      <p className="text-sm text-slate-600">성함과 전화번호는 이 문의에 답변하는 데 사용됩니다.</p>
      {activeListingId && <input type="hidden" name="listing_id" value={activeListingId} />}
      <div className="grid gap-4 sm:grid-cols-2">
        <label className="block text-sm font-medium text-slate-800">
          성함 <span aria-hidden="true">*</span>
          <input name="name" required maxLength={100} autoComplete="name" value={draft.name} onChange={(event) => update("name", event.target.value)} className="mt-1 block w-full rounded-lg border border-slate-300 px-3 py-2" />
        </label>
        <label className="block text-sm font-medium text-slate-800">
          전화번호 <span aria-hidden="true">*</span>
          <input name="phone" required maxLength={40} type="tel" autoComplete="tel" value={draft.phone} onChange={(event) => update("phone", event.target.value)} className="mt-1 block w-full rounded-lg border border-slate-300 px-3 py-2" />
        </label>
      </div>
      <label className="block text-sm font-medium text-slate-800">
        문의 종류 <span aria-hidden="true">*</span>
        <select name="topic" required value={draft.topic} onChange={(event) => update("topic", event.target.value)} className="mt-1 block w-full rounded-lg border border-slate-300 bg-white px-3 py-2">
          <option value="RENTAL">렌탈</option>
          <option value="USED_SALES">중고 구매</option>
          <option value="MAINTENANCE">정비</option>
          <option value="OTHER">기타</option>
        </select>
      </label>
      <label className="block text-sm font-medium text-slate-800">
        지역
        <input name="location" maxLength={120} autoComplete="address-level1" value={draft.location} onChange={(event) => update("location", event.target.value)} className="mt-1 block w-full rounded-lg border border-slate-300 px-3 py-2" />
      </label>
      <label className="block text-sm font-medium text-slate-800">
        문의 내용
        <textarea name="message" maxLength={2000} rows={4} value={draft.message} onChange={(event) => update("message", event.target.value)} className="mt-1 block w-full rounded-lg border border-slate-300 px-3 py-2" />
      </label>
      {(state.status === "error" || (state.status === "conflict" && !generalInquiry)) && <p role="alert" className="text-sm text-red-700">{state.message}</p>}
      {state.status === "conflict" && activeListingId && (
        <button type="button" onClick={() => setGeneralInquiry(true)} className="block text-sm font-semibold text-amber-800 underline underline-offset-4">
          일반 문의로 전환
        </button>
      )}
      {state.status === "conflict" && generalInquiry && <p ref={statusRef} role="status" tabIndex={-1} className="rounded text-sm text-slate-700 focus:outline-2 focus:outline-offset-2 focus:outline-amber-600">일반 문의로 전환했습니다. 내용을 확인한 뒤 문의 접수를 눌러 주세요.</p>}
      <button type="submit" disabled={pending} className="rounded-lg bg-slate-900 px-5 py-2.5 text-sm font-semibold text-white hover:bg-slate-700 disabled:opacity-60">
        {pending ? "접수 중…" : "문의 접수"}
      </button>
    </form>
  );
}
