"use client";

import React, { useEffect } from "react";
import { X, Command, Keyboard, Zap } from "lucide-react";

interface ShortcutModalProps {
  open: boolean;
  onClose: () => void;
}

export function ShortcutModal({ open, onClose }: ShortcutModalProps) {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && open) {
        onClose();
      }
      if (e.key === "?" && !open && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) {
        onClose(); // toggle
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open, onClose]);

  if (!open) return null;

  const shortcuts = [
    {
      group: "전역 탐색 & 커맨드 (Global Velocity)",
      items: [
        { key: "Cmd + K / Ctrl + K", desc: "글로벌 통합 커맨드 팔레트 (화면, 액션, 개체 즉시 검색)" },
        { key: "?", desc: "키보드 단축키 안내 창 토글" },
        { key: "Esc", desc: "열려있는 모든 검사창, 모달, 팔레트 닫기" },
        { key: "T", desc: "다크/라이트 테마 즉각 전환" },
      ],
    },
    {
      group: "Google Workspace / Excel 고밀도 그리드",
      items: [
        { key: "J / K", desc: "테이블 행 상/하 연속 이동 (Vim/Workspace 내비게이션)" },
        { key: "Enter", desc: "선택된 행의 상세 원장 / 개체 검사기 즉시 열기" },
        { key: "Ctrl + V", desc: "사원 관리 화면에서 엑셀 TSV 클립보드 행 즉시 일괄 파싱 및 등록" },
      ],
    },
    {
      group: "Discord / 슬랙식 동적 개체 연결",
      items: [
        { key: "# 또는 [", desc: "채팅/메신저 입력창에서 작업오더, 결재문서, 근태 개체 자동완성" },
        { key: "@", desc: "사원 및 담당자 멘션 자동완성" },
        { key: "Hover 개체 칩", desc: "게임식 HUD 미니카드 및 1-클릭 인라인 승인/착수 액션" },
      ],
    },
  ];

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4 animate-in fade-in duration-100">
      <div className="w-full max-w-xl rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl overflow-hidden animate-in zoom-in-95 duration-150">
        {/* 헤더 */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-[var(--border)] bg-gray-50/70">
          <div className="flex items-center gap-2.5">
            <span className="p-1.5 rounded-md bg-amber-500/20 text-amber-950 font-bold">
              <Keyboard className="w-4 h-4" />
            </span>
            <div>
              <h3 className="text-sm font-bold text-[var(--ink)]">키보드 벨로시티 단축키 가이드</h3>
              <p className="text-[11px] text-[var(--steel)]">
                마우스 없이 1-클릭 및 키보드만으로 모든 워크플로우를 완결할 수 있습니다.
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded-md text-[var(--steel)] hover:bg-[var(--muted)] cursor-pointer"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* 바디 */}
        <div className="p-5 space-y-5 max-h-[70vh] overflow-y-auto text-xs">
          {shortcuts.map((sec) => (
            <div key={sec.group} className="space-y-2">
              <h4 className="font-bold text-[var(--ink)] text-[11px] uppercase tracking-wider flex items-center gap-1.5">
                <Zap className="w-3 h-3 text-amber-600" />
                <span>{sec.group}</span>
              </h4>
              <div className="space-y-1.5">
                {sec.items.map((it) => (
                  <div
                    key={it.key}
                    className="flex items-center justify-between p-2 rounded-md bg-[var(--canvas)] border border-[var(--border)] hover:border-[var(--steel)] transition-colors"
                  >
                    <span className="text-[var(--ink)]">{it.desc}</span>
                    <kbd className="px-2 py-0.5 rounded bg-[var(--surface)] border border-[var(--border)] text-[11px] font-mono font-bold text-amber-900 shadow-2xs">
                      {it.key}
                    </kbd>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>

        {/* 푸터 */}
        <div className="px-5 py-3 border-t border-[var(--border)] bg-gray-50/50 flex justify-between items-center text-xs">
          <span className="text-[11px] text-[var(--steel)]">Acme Group / Oyatie Console High-Velocity HUD</span>
          <button
            type="button"
            onClick={onClose}
            className="px-3 py-1.5 rounded-md bg-[var(--muted)] text-[var(--ink)] hover:bg-gray-200 font-semibold cursor-pointer"
          >
            닫기 (Esc)
          </button>
        </div>
      </div>
    </div>
  );
}
