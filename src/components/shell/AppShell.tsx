"use client";

import React from "react";
import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";
import { CommsRail } from "./CommsRail";
import { CommandPalette } from "../ui/CommandPalette";
import { ToastContainer } from "../ui/Toast";
import { ShortcutModal } from "../ui/ShortcutModal";
import { ErrorBoundary } from "../ui/ErrorBoundary";
import { I18nProvider } from "@/lib/i18n";
import { useAppStore } from "@/lib/store";

export function AppShell({ children }: { children: React.ReactNode }) {
  const viewAsRole = useAppStore((s) => s.viewAsRole);
  const shortcutModalOpen = useAppStore((s) => s.shortcutModalOpen);
  const setShortcutModalOpen = useAppStore((s) => s.setShortcutModalOpen);
  const toggleTheme = useAppStore((s) => s.toggleTheme);

  // 전역 ? 키 감지
  React.useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (
        e.key === "?" &&
        !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)
      ) {
        e.preventDefault();
        setShortcutModalOpen(!shortcutModalOpen);
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [shortcutModalOpen, setShortcutModalOpen]);

  return (
    <ErrorBoundary>
      <I18nProvider>
        <div className="flex h-screen w-screen overflow-hidden bg-[var(--canvas)] text-[var(--ink)] antialiased">
          {/* 토스트 HUD 알림 */}
          <ToastContainer />

      {/* 키보드 단축키 가이드 모달 */}
      <ShortcutModal
        open={shortcutModalOpen}
        onClose={() => setShortcutModalOpen(false)}
      />

      {/* 글로벌 Command Palette 모달 */}
      <CommandPalette />

      {/* 좌측 사이드바 (240px / 64px) */}
      <Sidebar />

      {/* 중앙 메인 작업 영역 */}
      <div className="flex flex-col flex-1 min-w-0 overflow-hidden">
        {/* 플랫폼 오퍼레이터 View As 배너 (역할 전환 시 실시간 고지) */}
        {viewAsRole !== "회장단" && (
          <div className="flex items-center justify-between px-4 py-1 bg-amber-500/15 border-b border-amber-500/30 text-amber-950 text-[11px] font-medium shrink-0">
            <div className="flex items-center gap-2">
              <span className="inline-block w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
              <span>
                <strong>권한 시뮬레이션 모드:</strong> 현재 <strong>[{viewAsRole}]</strong> 권한으로 화면을 열람 중입니다. (감사 이벤트 자동 태깅)
              </span>
            </div>
            <span className="text-[10px] font-mono text-amber-800">PALANTIR CEDAR PBAC ACTIVE</span>
          </div>
        )}

        {/* 상단바 */}
        <Topbar />

        <p className="shrink-0 border-b border-amber-200 bg-amber-50 px-4 py-2 text-xs text-amber-950">
          시연용 화면입니다. 새 변경 내용은 서버에 저장되지 않아 새로고침하면 사라집니다. 이전 브라우저 저장 데이터는 자동으로 불러오거나 삭제하지 않습니다.
        </p>

        {/* 주 작업 캔버스 */}
        <main className="flex-1 overflow-y-auto p-4 md:p-6 min-w-0">
          <div className="max-w-7xl mx-auto space-y-5">{children}</div>
        </main>
      </div>

      {/* 우측 협업/맥락 레일 (Slack-style 320px) */}
      <CommsRail />
        </div>
      </I18nProvider>
    </ErrorBoundary>
  );
}
