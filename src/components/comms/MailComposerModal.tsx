"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { Button } from "@/components/ui/Button";
import { ObjectLink } from "@/components/ui/ObjectLink";
import {
  Send,
  X,
  Mail,
  User,
  Paperclip,
  FileCheck2,
  Wrench,
  FileText,
  DollarSign,
  AlertCircle,
} from "lucide-react";

interface MailComposerModalProps {
  isOpen: boolean;
  onClose: () => void;
  initialRecipient?: string;
  initialRecipientEmail?: string;
  initialSubject?: string;
  initialBody?: string;
  initialLinkedObject?: string;
}

export function MailComposerModal({
  isOpen,
  onClose,
  initialRecipient = "",
  initialRecipientEmail = "",
  initialSubject = "",
  initialBody = "",
  initialLinkedObject = "",
}: MailComposerModalProps) {
  const employees = useAppStore((s) => s.employees);
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const contracts = useAppStore((s) => s.contracts);
  const sendEmail = useAppStore((s) => s.sendEmail);

  const [recipientName, setRecipientName] = useState(initialRecipient);
  const [recipientEmail, setRecipientEmail] = useState(initialRecipientEmail);
  const [subject, setSubject] = useState(initialSubject);
  const [body, setBody] = useState(initialBody);
  const [linkedObject, setLinkedObject] = useState(initialLinkedObject);
  const [validationError, setValidationError] = useState<string | null>(null);

  if (!isOpen) return null;

  const handleSelectEmployee = (empId: string) => {
    const emp = employees.find((e) => e.id === empId);
    if (emp) {
      setRecipientName(`${emp.name} (${emp.dept} ${emp.role})`);
      setRecipientEmail(emp.email || `${emp.id}@oyatie.com`);
    }
  };

  const handleApplyPreset = (prefix: string) => {
    if (!subject.startsWith("[")) {
      setSubject(`${prefix} ${subject}`);
    } else {
      setSubject(`${prefix} ${subject.replace(/^\[[^\]]+\]\s*/, "")}`);
    }
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!recipientEmail.trim()) {
      setValidationError("수신자 이메일 주소를 입력하거나 임직원을 선택하십시오.");
      return;
    }
    if (!subject.trim()) {
      setValidationError("메일 제목을 입력하십시오.");
      return;
    }
    if (!body.trim()) {
      setValidationError("메일 본문 내용을 입력하십시오.");
      return;
    }

    sendEmail({
      recipient: recipientName || recipientEmail,
      recipientEmail,
      subject,
      body,
      linkedObject: linkedObject || undefined,
    });

    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
      <div className="w-full max-w-2xl rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl overflow-hidden flex flex-col animate-in fade-in zoom-in-95 duration-150 max-h-[90vh]">
        {/* Header */}
        <div className="px-5 py-3.5 border-b border-[var(--border)] flex items-center justify-between bg-gradient-to-r from-amber-500/10 via-transparent to-transparent">
          <div className="flex items-center gap-2">
            <span className="p-1.5 rounded-lg bg-amber-500/10 text-amber-600 dark:text-amber-400">
              <Mail size={18} />
            </span>
            <div>
              <h3 className="text-sm font-bold text-[var(--ink)]">신규 사내 업무 메일 발송</h3>
              <p className="text-[11px] text-[var(--steel)]">
                엔터프라이즈 도메인 개체와 연계된 추적 가능 공식 메일을 작성합니다.
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
        <form onSubmit={handleSubmit} className="p-5 space-y-4 overflow-y-auto text-xs flex-1">
          {validationError && (
            <div className="p-2.5 rounded-md bg-red-50 border border-red-200 text-red-700 flex items-center gap-2">
              <AlertCircle size={14} />
              <span>{validationError}</span>
            </div>
          )}

          {/* 수신자 선택 */}
          <div className="space-y-1.5">
            <label className="block font-semibold text-[var(--ink)]">
              수신인 (사내 임직원 검색 또는 직접 입력):
            </label>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              <select
                onChange={(e) => handleSelectEmployee(e.target.value)}
                defaultValue=""
                className="px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none"
              >
                <option value="" disabled>
                  -- 사내 임직원 명부에서 선택 --
                </option>
                {employees.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.name} ({e.dept} · {e.role})
                  </option>
                ))}
              </select>

              <input
                type="email"
                value={recipientEmail}
                onChange={(e) => setRecipientEmail(e.target.value)}
                placeholder="수신 이메일 주소 (e.g. name@oyatie.com)"
                className="px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-mono"
              />
            </div>
          </div>

          {/* 제목 및 프리셋 */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <label className="block font-semibold text-[var(--ink)]">메일 제목:</label>
              <div className="flex items-center gap-1">
                <span className="text-[10px] text-[var(--faint)]">말머리:</span>
                <button
                  type="button"
                  onClick={() => handleApplyPreset("[품의협의]")}
                  className="px-1.5 py-0.5 rounded bg-blue-100 text-blue-800 text-[10px] hover:bg-blue-200 cursor-pointer"
                >
                  [품의협의]
                </button>
                <button
                  type="button"
                  onClick={() => handleApplyPreset("[정비보고]")}
                  className="px-1.5 py-0.5 rounded bg-amber-100 text-amber-800 text-[10px] hover:bg-amber-200 cursor-pointer"
                >
                  [정비보고]
                </button>
                <button
                  type="button"
                  onClick={() => handleApplyPreset("[급여확인]")}
                  className="px-1.5 py-0.5 rounded bg-emerald-100 text-emerald-800 text-[10px] hover:bg-emerald-200 cursor-pointer"
                >
                  [급여확인]
                </button>
                <button
                  type="button"
                  onClick={() => handleApplyPreset("[긴급공지]")}
                  className="px-1.5 py-0.5 rounded bg-red-100 text-red-800 text-[10px] hover:bg-red-200 cursor-pointer"
                >
                  [긴급공지]
                </button>
              </div>
            </div>
            <input
              type="text"
              value={subject}
              onChange={(e) => setSubject(e.target.value)}
              placeholder="업무 제목을 입력하십시오."
              className="w-full px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-medium"
            />
          </div>

          {/* 연계 업무 개체 선택 */}
          <div className="space-y-1.5">
            <label className="block font-semibold text-[var(--ink)]">
              연계 업무 개체 (Business Entity Link):
            </label>
            <div className="flex items-center gap-2">
              <select
                value={linkedObject}
                onChange={(e) => setLinkedObject(e.target.value)}
                className="flex-1 px-2.5 py-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-mono"
              >
                <option value="">-- 연계할 업무 개체 없음 --</option>
                <optgroup label="전자결재 문서 (AP-)">
                  {approvals.map((a) => (
                    <option key={a.id} value={a.code}>
                      {a.code} - {a.title} ({a.status})
                    </option>
                  ))}
                </optgroup>
                <optgroup label="정비/작업 오더 (WO-)">
                  {workOrders.map((w) => (
                    <option key={w.id} value={w.code}>
                      {w.code} - {w.title} ({w.status})
                    </option>
                  ))}
                </optgroup>
                <optgroup label="도급/수주 계약 (C-)">
                  {contracts.map((c) => (
                    <option key={c.id} value={c.code}>
                      {c.code} - {c.title}
                    </option>
                  ))}
                </optgroup>
                <optgroup label="임직원 (EMP-)">
                  {employees.slice(0, 10).map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.id} - {e.name} ({e.dept})
                    </option>
                  ))}
                </optgroup>
              </select>
              {linkedObject && <ObjectLink code={linkedObject} />}
            </div>
            <p className="text-[10px] text-[var(--faint)]">
              개체를 연결하면 메일 수신자가 실시간 상태 카드 및 1-클릭 전결/배차 액션을 바로 열람할 수 있습니다.
            </p>
          </div>

          {/* 본문 내용 */}
          <div className="space-y-1.5">
            <label className="block font-semibold text-[var(--ink)]">메일 본문:</label>
            <textarea
              rows={8}
              value={body}
              onChange={(e) => setBody(e.target.value)}
              placeholder="업무 내용, 요청 사항, 근거 법령 및 현장 상황을 상세히 서술하십시오."
              className="w-full p-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] focus:outline-none font-sans leading-relaxed resize-none"
            />
          </div>

          {/* Footer Controls */}
          <div className="pt-3 border-t border-[var(--border)] flex items-center justify-between">
            <div className="text-[11px] text-[var(--steel)]">
              발신인: <span className="font-semibold text-[var(--ink)]">박지영 수석 (인사노무팀)</span>
            </div>
            <div className="flex items-center gap-2">
              <Button type="button" variant="ghost" size="sm" onClick={onClose}>
                취소
              </Button>
              <Button type="submit" variant="brand" size="sm" leftIcon={<Send size={14} />}>
                메일 발송
              </Button>
            </div>
          </div>
        </form>
      </div>
    </div>
  );
}
