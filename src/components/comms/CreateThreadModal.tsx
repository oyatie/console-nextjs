"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { ObjectLink } from "@/components/ui/ObjectLink";
import {
  MessageSquare,
  Hash,
  Layers,
  X,
  Plus,
  AlertCircle,
  FileCheck2,
  Wrench,
  FileText,
} from "lucide-react";

interface CreateThreadModalProps {
  isOpen: boolean;
  onClose: () => void;
  onCreated: (threadId: string) => void;
}

export function CreateThreadModal({ isOpen, onClose, onCreated }: CreateThreadModalProps) {
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const contracts = useAppStore((s) => s.contracts);
  const createThread = useAppStore((s) => s.createThread);

  const [threadType, setThreadType] = useState<"object" | "channel">("object");
  const [selectedObject, setSelectedObject] = useState<string>(approvals[0]?.code || "");
  const [title, setTitle] = useState("");
  const [initialMessage, setInitialMessage] = useState("");
  const [error, setError] = useState<string | null>(null);

  if (!isOpen) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (threadType === "channel" && !title.trim()) {
      setError("채널명을 입력하십시오 (예: #현장안전관리, #7월급여정산).");
      return;
    }
    if (threadType === "object" && !selectedObject.trim()) {
      setError("연계할 도메인 개체를 선택하십시오.");
      return;
    }

    const created = createThread({
      title: threadType === "channel" ? title : `${selectedObject} ${title || "업무 협의"}`,
      objectRef: threadType === "object" ? selectedObject : undefined,
      initialMessage: initialMessage.trim() || undefined,
    });

    onCreated(created.id);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
      <div className="w-full max-w-lg rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl overflow-hidden flex flex-col animate-in fade-in zoom-in-95 duration-150">
        {/* Header */}
        <div className="px-5 py-4 border-b border-[var(--border)] flex items-center justify-between bg-gradient-to-r from-amber-500/10 via-transparent to-transparent">
          <div className="flex items-center gap-2">
            <span className="p-1.5 rounded-lg bg-amber-500/10 text-amber-600 dark:text-amber-400">
              <MessageSquare size={18} />
            </span>
            <div>
              <h3 className="text-sm font-bold text-[var(--ink)]">새 협업 채널 및 스레드 개설</h3>
              <p className="text-[11px] text-[var(--steel)]">
                엔티티 중심의 맥락 스레드 또는 조직 공용 채널을 생성합니다.
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded-md text-[var(--steel)] hover:text-[var(--ink)] hover:bg-[var(--muted)] cursor-pointer"
          >
            <X size={16} />
          </button>
        </div>

        {/* Form Body */}
        <form onSubmit={handleSubmit} className="p-5 space-y-4 text-xs">
          {error && (
            <div className="p-2.5 rounded-md bg-red-50 border border-red-200 text-red-700 flex items-center gap-2">
              <AlertCircle size={14} />
              <span>{error}</span>
            </div>
          )}

          {/* 스레드 타입 선택 */}
          <div className="space-y-1.5">
            <label className="block font-semibold text-[var(--ink)]">채널 유형:</label>
            <div className="grid grid-cols-2 gap-2">
              <button
                type="button"
                onClick={() => {
                  setThreadType("object");
                  setError(null);
                }}
                className={`p-3 rounded-lg border text-left flex items-start gap-2.5 cursor-pointer transition-all ${
                  threadType === "object"
                    ? "border-amber-400 bg-amber-50/80 text-amber-950 font-bold ring-1 ring-amber-400"
                    : "border-[var(--border)] bg-gray-50/50 hover:bg-gray-100/50 text-[var(--steel)]"
                }`}
              >
                <Layers className="w-4 h-4 text-amber-600 shrink-0 mt-0.5" />
                <div>
                  <div className="font-semibold text-xs text-[var(--ink)]">개체 맥락 스레드</div>
                  <div className="text-[10px] text-[var(--faint)] mt-0.5">
                    AP, WO, C- 등 특정 품의/오더에 묶인 협업 방
                  </div>
                </div>
              </button>

              <button
                type="button"
                onClick={() => {
                  setThreadType("channel");
                  setError(null);
                }}
                className={`p-3 rounded-lg border text-left flex items-start gap-2.5 cursor-pointer transition-all ${
                  threadType === "channel"
                    ? "border-amber-400 bg-amber-50/80 text-amber-950 font-bold ring-1 ring-amber-400"
                    : "border-[var(--border)] bg-gray-50/50 hover:bg-gray-100/50 text-[var(--steel)]"
                }`}
              >
                <Hash className="w-4 h-4 text-amber-600 shrink-0 mt-0.5" />
                <div>
                  <div className="font-semibold text-xs text-[var(--ink)]">조직 공용 채널</div>
                  <div className="text-[10px] text-[var(--faint)] mt-0.5">
                    부서, 프로젝트 또는 주제별 상설 대화방
                  </div>
                </div>
              </button>
            </div>
          </div>

          {/* 개체 맥락 스레드인 경우 개체 선택 */}
          {threadType === "object" && (
            <div className="space-y-1.5">
              <label className="block font-semibold text-[var(--ink)]">연계할 업무 개체:</label>
              <select
                value={selectedObject}
                onChange={(e) => setSelectedObject(e.target.value)}
                className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-mono"
              >
                <optgroup label="전자결재 문서 (AP-)">
                  {approvals.map((a) => (
                    <option key={a.id} value={a.code}>
                      {a.code} - {a.title} ({a.status})
                    </option>
                  ))}
                </optgroup>
                <optgroup label="작업/정비 오더 (WO-)">
                  {workOrders.map((w) => (
                    <option key={w.id} value={w.code}>
                      {w.code} - {w.title} ({w.status})
                    </option>
                  ))}
                </optgroup>
                <optgroup label="도급 계약 (C-)">
                  {contracts.map((c) => (
                    <option key={c.id} value={c.code}>
                      {c.code} - {c.title}
                    </option>
                  ))}
                </optgroup>
              </select>
            </div>
          )}

          {/* 채널명 / 스레드 제목 */}
          <div className="space-y-1.5">
            <label className="block font-semibold text-[var(--ink)]">
              {threadType === "channel" ? "채널명 (Channel Title):" : "스레드 부제 (선택사항):"}
            </label>
            <input
              type="text"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder={
                threadType === "channel"
                  ? "e.g. #인천물류-안전관리위원회"
                  : "e.g. 야간 긴급 가동 대책 회의"
              }
              className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-medium"
            />
          </div>

          {/* 최초 시작 메시지 */}
          <div className="space-y-1.5">
            <label className="block font-semibold text-[var(--ink)]">최초 개설 메시지 (선택사항):</label>
            <textarea
              rows={3}
              value={initialMessage}
              onChange={(e) => setInitialMessage(e.target.value)}
              placeholder="스레드를 시작하면서 참여자들에게 남길 안내 또는 첫 질문을 적어주십시오."
              className="w-full p-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none resize-none font-sans leading-relaxed"
            />
          </div>

          {/* Footer Controls */}
          <div className="pt-3 border-t border-[var(--border)] flex items-center justify-end gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={onClose}>
              취소
            </Button>
            <Button type="submit" variant="brand" size="sm" leftIcon={<Plus size={14} />}>
              채널 생성
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
