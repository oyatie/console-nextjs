"use client";

import React, { useState } from "react";
import Link from "next/link";
import { clsx } from "clsx";
import { useAppStore } from "@/lib/store";
import {
  MessageSquare,
  Layers,
  Bell,
  X,
  Send,
  Maximize2,
  ShieldCheck,
  Clock,
  ExternalLink,
} from "lucide-react";
import { StatusChip } from "../ui/StatusChip";
import { ObjectLink } from "../ui/ObjectLink";
import { DynamicText } from "../ui/DynamicText";
import { EntityMentionPicker } from "../ui/EntityMentionPicker";

export function CommsRail() {
  const open = useAppStore((s) => s.rightRailOpen);
  const tab = useAppStore((s) => s.rightRailTab);
  const setTab = useAppStore((s) => s.toggleRightRail);
  const toggleRail = useAppStore((s) => s.toggleRightRail);
  const activeCode = useAppStore((s) => s.activeObjectCode);
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const employees = useAppStore((s) => s.employees);
  const threads = useAppStore((s) => s.threads);
  const messages = useAppStore((s) => s.messages);
  const sendMessage = useAppStore((s) => s.sendMessage);

  const [inputVal, setInputVal] = useState("");
  const [mentionTrigger, setMentionTrigger] = useState<"#" | "@" | "[" | null>(null);
  const [mentionQuery, setMentionQuery] = useState("");

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const val = e.target.value;
    setInputVal(val);
    const match = val.match(/([#@\[])([\w가-힣]*)$/);
    if (match) {
      setMentionTrigger(match[1] as "#" | "@" | "[");
      setMentionQuery(match[2] || "");
    } else {
      setMentionTrigger(null);
      setMentionQuery("");
    }
  };

  const handleMentionSelect = (tokenText: string) => {
    if (!mentionTrigger) return;
    const lastIndex = inputVal.lastIndexOf(mentionTrigger);
    if (lastIndex !== -1) {
      const updated = inputVal.substring(0, lastIndex) + tokenText + " ";
      setInputVal(updated);
    }
    setMentionTrigger(null);
    setMentionQuery("");
  };

  if (!open) return null;

  // 현재 활성 개체 검사 대상 찾기
  const attendance = useAppStore((s) => s.attendance);
  const payslips = useAppStore((s) => s.payslips);
  const payrollRun = useAppStore((s) => s.payrollRun);

  let inspectedObject: any = null;
  if (activeCode?.startsWith("AP-")) {
    inspectedObject = approvals.find((a) => a.code === activeCode);
  } else if (activeCode?.startsWith("WO-")) {
    inspectedObject = workOrders.find((w) => w.code === activeCode);
  } else if (activeCode?.startsWith("AT-")) {
    const att = attendance.find((a) => a.exceptionCode === activeCode);
    if (att) {
      inspectedObject = {
        code: activeCode,
        title: `${att.employeeName} — 근태 52시간/지각 예외`,
        status: att.status,
        site: att.site,
        drafterName: att.employeeName,
        workedHours: att.workedHours,
        otHours: att.otHours,
        linkedObjects: ["PR-2026-07"],
      };
    }
  } else if (activeCode?.startsWith("EMP-")) {
    const emp = employees.find((e) => e.code === activeCode);
    if (emp) {
      inspectedObject = {
        code: activeCode,
        title: `${emp.name} (${emp.dept} · ${emp.role})`,
        status: emp.status,
        site: emp.site,
        drafterName: emp.name,
        role: emp.role,
        position: emp.position,
        salary: `${emp.baseSalary.toLocaleString()}원`,
        linkedObjects: [`PS-${emp.code.replace("EMP-", "")}`],
      };
    }
  } else if (activeCode?.startsWith("PS-")) {
    const ps = payslips.find((p) => p.code === activeCode);
    if (ps) {
      inspectedObject = {
        code: activeCode,
        title: `${ps.employeeName} — 2026년 7월 급여명세서`,
        status: "지급대기",
        drafterName: ps.employeeName,
        doaAmount: ps.netPay,
        linkedObjects: ["PR-2026-07"],
      };
    }
  } else if (activeCode?.startsWith("C-")) {
    inspectedObject = {
      code: activeCode,
      title: `${activeCode} 현장 수주 및 도급 운영 계약`,
      status: "게시",
      site: "인천/평택/당진 현장",
      drafterName: "경영기획실",
      doaAmount: 1200000000,
      linkedObjects: ["AP-3125", "WO-2641"],
    };
  } else if (activeCode?.startsWith("PR-")) {
    inspectedObject = {
      code: activeCode,
      title: payrollRun.title,
      status: payrollRun.status,
      site: "그룹 전체",
      drafterName: "인사노무팀",
      doaAmount: payrollRun.totalNet,
      linkedObjects: ["AP-3124"],
    };
  }

  // 현재 활성 스레드 찾기
  const activeThread =
    threads.find((t) => t.objectRef === activeCode) || threads[0];
  const threadMessages = activeThread ? messages[activeThread.id] || [] : [];

  const handleSend = (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputVal.trim() || !activeThread) return;
    sendMessage(activeThread.id, inputVal.trim());
    setInputVal("");
  };

  return (
    <aside className="w-80 border-l border-[var(--border)] bg-[var(--surface)] flex flex-col shrink-0 z-20 transition-all">
      {/* 레일 상단 탭 헤더 */}
      <div className="flex items-center justify-between border-b border-[var(--border)] px-3 h-10 select-none">
        <div className="flex items-center gap-2">
          <button
            onClick={() => setTab("thread")}
            className={clsx(
              "text-xs px-2 py-1 rounded font-medium transition-colors cursor-pointer",
              tab === "thread"
                ? "bg-amber-100 text-amber-900 font-bold"
                : "text-[var(--steel)] hover:bg-[var(--muted)]"
            )}
          >
            스레드
          </button>
          <button
            onClick={() => setTab("inspector")}
            className={clsx(
              "text-xs px-2 py-1 rounded font-medium transition-colors cursor-pointer",
              tab === "inspector"
                ? "bg-amber-100 text-amber-900 font-bold"
                : "text-[var(--steel)] hover:bg-[var(--muted)]"
            )}
          >
            개체 검사기
          </button>
          <button
            onClick={() => setTab("chat")}
            className={clsx(
              "text-xs px-2 py-1 rounded font-medium transition-colors cursor-pointer",
              tab === "chat"
                ? "bg-amber-100 text-amber-900 font-bold"
                : "text-[var(--steel)] hover:bg-[var(--muted)]"
            )}
          >
            채널
          </button>
        </div>

        <div className="flex items-center gap-1">
          <Link
            href="/comms/messenger"
            className="p-1 rounded text-[var(--steel)] hover:bg-[var(--muted)]"
            title="메신저 전체화면 승격 (Slack Rail ↔ Main)"
          >
            <Maximize2 className="w-3.5 h-3.5" />
          </Link>
          <button
            onClick={() => toggleRail()}
            className="p-1 rounded text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
            title="닫기"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* 탭 1: 개체 맥락 스레드 (Slack-style Contextual Discussion) */}
      {tab === "thread" && (
        <div className="flex-1 flex flex-col overflow-hidden">
          {activeThread ? (
            <>
              {/* 스레드 개요 배너 */}
              <div className="p-3 border-b border-[var(--border)] bg-gray-50/70">
                <div className="flex items-center justify-between">
                  <span className="font-mono text-xs font-bold text-blue-700">
                    {activeThread.objectRef || "일반 스레드"}
                  </span>
                  <span className="text-[10px] text-[var(--faint)]">{activeThread.updatedAt}</span>
                </div>
                <h4 className="text-xs font-semibold text-[var(--ink)] mt-1 truncate">
                  {activeThread.title}
                </h4>
                <div className="flex items-center gap-1 mt-1 text-[10px] text-[var(--steel)]">
                  <span>참여자: {activeThread.participantNames.join(", ")}</span>
                </div>
              </div>

              {/* 메시지 리스트 */}
              <div className="flex-1 overflow-y-auto p-3 space-y-3">
                {threadMessages.length === 0 ? (
                  <div className="text-center py-8 text-xs text-[var(--faint)]">
                    대화 기록이 없습니다.
                  </div>
                ) : (
                  threadMessages.map((m) => (
                    <div key={m.id} className="flex items-start gap-2 text-xs group">
                      <div className="w-6 h-6 rounded bg-amber-500/20 text-amber-900 font-bold flex items-center justify-center text-[10px] shrink-0">
                        {m.avatarText}
                      </div>
                      <div className="flex-1 space-y-1">
                        <div className="flex items-center justify-between">
                          <span className="font-semibold text-[var(--ink)]">{m.authorName}</span>
                          <span className="text-[10px] text-[var(--faint)] font-mono">{m.timestamp}</span>
                        </div>
                        <div className="text-[var(--ink)] leading-relaxed text-xs">
                          <DynamicText text={m.text} />
                        </div>
                        {m.linkedCodes && (
                          <div className="flex items-center gap-1 mt-1">
                            {m.linkedCodes.map((code) => (
                              <ObjectLink key={code} code={code} />
                            ))}
                          </div>
                        )}
                      </div>
                    </div>
                  ))
                )}
              </div>

              {/* 메시지 입력 폼 */}
              <form onSubmit={handleSend} className="relative p-2 border-t border-[var(--border)] bg-[var(--surface)]">
                {mentionTrigger && (
                  <EntityMentionPicker
                    query={mentionQuery}
                    triggerType={mentionTrigger}
                    onSelect={handleMentionSelect}
                    onClose={() => setMentionTrigger(null)}
                  />
                )}
                <div className="flex items-center gap-2 border border-[var(--border)] rounded-md px-2.5 py-1.5 bg-[var(--canvas)] focus-within:border-[var(--steel)]">
                  <input
                    type="text"
                    value={inputVal}
                    onChange={handleInputChange}
                    placeholder="메시지 입력... (@멘션, #개체)"
                    className="flex-1 text-xs bg-transparent focus:outline-none text-[var(--ink)] placeholder:text-[var(--faint)]"
                  />
                  <button
                    type="submit"
                    disabled={!inputVal.trim()}
                    className="text-amber-600 disabled:opacity-30 hover:text-amber-700 cursor-pointer"
                  >
                    <Send className="w-3.5 h-3.5" />
                  </button>
                </div>
              </form>
            </>
          ) : (
            <div className="flex-1 flex items-center justify-center p-4 text-xs text-[var(--faint)]">
              선택된 스레드가 없습니다.
            </div>
          )}
        </div>
      )}

      {/* 탭 2: Palantir 온톨로지 개체 검사기 (Inspector) */}
      {tab === "inspector" && (
        <div className="flex-1 overflow-y-auto p-3 space-y-4 text-xs">
          {inspectedObject ? (
            <>
              <div>
                <div className="flex items-center justify-between">
                  <span className="font-mono text-sm font-bold text-[var(--signal-deep)]">
                    {inspectedObject.code}
                  </span>
                  <StatusChip label={inspectedObject.status} />
                </div>
                <h3 className="font-semibold text-sm text-[var(--ink)] mt-1">
                  {inspectedObject.title}
                </h3>
              </div>

              {/* Cedar PBAC 인가 판정 상태 */}
              <div className="p-2 rounded border border-emerald-200 bg-emerald-50 text-[11px] text-emerald-900 space-y-1">
                <div className="flex items-center gap-1 font-semibold">
                  <ShieldCheck className="w-3.5 h-3.5 text-emerald-700" />
                  <span>Cedar PBAC: PERMIT</span>
                </div>
                <p className="text-[10px] text-emerald-800">
                  현재 사용자 권한으로 본 개체의 열람 및 승인/반려 실행이 인가되었습니다.
                </p>
              </div>

              {/* 개체 속성 테이블 */}
              <div className="border border-[var(--border)] rounded divide-y divide-[var(--border)]">
                <div className="px-2.5 py-1.5 flex justify-between bg-gray-50 font-semibold text-[11px] text-[var(--steel)]">
                  <span>온톨로지 속성</span>
                  <span>값</span>
                </div>
                {inspectedObject.drafterName && (
                  <div className="px-2.5 py-1.5 flex justify-between">
                    <span className="text-[var(--steel)]">기안자</span>
                    <span className="font-medium">{inspectedObject.drafterName}</span>
                  </div>
                )}
                {inspectedObject.assignedTo && (
                  <div className="px-2.5 py-1.5 flex justify-between">
                    <span className="text-[var(--steel)]">배차 담당</span>
                    <span className="font-medium">{inspectedObject.assignedTo}</span>
                  </div>
                )}
                {inspectedObject.site && (
                  <div className="px-2.5 py-1.5 flex justify-between">
                    <span className="text-[var(--steel)]">발생 현장</span>
                    <span className="font-medium">{inspectedObject.site}</span>
                  </div>
                )}
                {inspectedObject.doaAmount && (
                  <div className="px-2.5 py-1.5 flex justify-between">
                    <span className="text-[var(--steel)]">품의 금액</span>
                    <span className="font-mono font-bold">{inspectedObject.doaAmount.toLocaleString()}원</span>
                  </div>
                )}
                <div className="px-2.5 py-1.5 flex justify-between">
                  <span className="text-[var(--steel)]">생성 시각</span>
                  <span className="font-mono text-[10px] text-[var(--faint)]">{inspectedObject.createdAt || "2026-07-03"}</span>
                </div>
              </div>

              {/* 연결된 개체 관계망 (Palantir Graph Links) */}
              <div className="space-y-1.5">
                <span className="font-semibold text-[11px] text-[var(--steel)]">연결된 개체 (Linked Objects)</span>
                <div className="flex flex-wrap gap-1.5">
                  {(inspectedObject.linkedObjects || ["C-207", "AT-0703-01"]).map((c: string) => (
                    <ObjectLink key={c} code={c} />
                  ))}
                </div>
              </div>
            </>
          ) : (
            <div className="text-center py-12 text-[var(--faint)]">
              좌측 화면의 개체 칩을 클릭하면 여기에 상세 정보가 노출됩니다.
            </div>
          )}
        </div>
      )}

      {/* 탭 3: 메신저 채널 목록 */}
      {tab === "chat" && (
        <div className="flex-1 overflow-y-auto p-2 space-y-1 text-xs">
          <div className="px-2 py-1 text-[10px] font-semibold text-[var(--faint)] uppercase">
            참여 채널
          </div>
          {threads.map((t) => (
            <div
              key={t.id}
              onClick={() => setTab("thread")}
              className="p-2 rounded-md hover:bg-[var(--muted)] cursor-pointer transition-colors space-y-0.5"
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold truncate text-[var(--ink)]">{t.title}</span>
                <span className="text-[10px] text-[var(--faint)]">{t.updatedAt}</span>
              </div>
              <p className="text-[11px] text-[var(--steel)] truncate">{t.lastMessage}</p>
            </div>
          ))}
        </div>
      )}
    </aside>
  );
}
