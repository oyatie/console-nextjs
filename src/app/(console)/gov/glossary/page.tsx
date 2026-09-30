// page.tsx — Enterprise Terminology & Regulatory Glossary Console
"use client";

import React, { useState, useMemo } from "react";
import { useGlossary, GlossaryEntry } from "@/lib/i18n";
import {
  BookOpen,
  Search,
  Scale,
  Globe2,
  CheckCircle2,
  ExternalLink,
  Shield,
  Layers,
  FileText,
  Filter,
  Sparkles,
} from "lucide-react";
import { StatusChip } from "@/components/ui/StatusChip";
import { Button } from "@/components/ui/Button";

const CATEGORY_LABELS: Record<string, { label: string; icon: any; tone: "ok" | "info" | "warn" | "neutral" | "danger" }> = {
  all: { label: "전체 도메인", icon: Layers, tone: "neutral" },
  common: { label: "공통 UI/액션", icon: Sparkles, tone: "neutral" },
  org: { label: "조직 / 법인", icon: Layers, tone: "info" },
  rbac: { label: "직무권한 (RBAC)", icon: Shield, tone: "warn" },
  hr: { label: "인사 / 발령", icon: FileText, tone: "info" },
  attendance: { label: "근태 / 주52시간", icon: Scale, tone: "warn" },
  payroll: { label: "급여 / 4대보험", icon: Scale, tone: "ok" },
  ops: { label: "현장 / 작업오더", icon: Layers, tone: "info" },
  approvals: { label: "전자결재 (DoA)", icon: Shield, tone: "ok" },
  gov: { label: "거버넌스 / 감사", icon: Scale, tone: "danger" },
};

export default function GlossaryPage() {
  const { allTerms, locale, setLocale, t } = useGlossary();
  const [selectedCategory, setSelectedCategory] = useState<string>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedEntry, setSelectedEntry] = useState<GlossaryEntry | null>(null);

  const termList = useMemo(() => Object.values(allTerms), [allTerms]);

  // Filtered terms
  const filteredTerms = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return termList.filter((entry) => {
      if (selectedCategory !== "all" && entry.category !== selectedCategory) {
        return false;
      }
      if (!q) return true;
      return (
        entry.key.toLowerCase().includes(q) ||
        entry.ko.toLowerCase().includes(q) ||
        entry.en.toLowerCase().includes(q) ||
        (entry.legalBasis && entry.legalBasis.toLowerCase().includes(q)) ||
        entry.description.toLowerCase().includes(q)
      );
    });
  }, [termList, selectedCategory, searchQuery]);

  // Statistics
  const stats = useMemo(() => {
    const total = termList.length;
    const withLaw = termList.filter((e) => e.legalBasis).length;
    const categoriesCount = new Set(termList.map((e) => e.category)).size;
    return { total, withLaw, categoriesCount };
  }, [termList]);

  return (
    <div className="space-y-6">
      {/* 1. Header & Locale Toggle */}
      <div className="flex flex-col md:flex-row md:items-center justify-between gap-4 pb-4 border-b border-[var(--border)]">
        <div>
          <div className="flex items-center gap-2">
            <span className="p-1.5 rounded-lg bg-[var(--brand-subtle)] text-[var(--brand)]">
              <BookOpen className="w-5 h-5" />
            </span>
            <h1 className="text-xl font-bold tracking-tight">엔터프라이즈 용어 사전 & 법률 기준 거버넌스</h1>
          </div>
          <p className="text-sm text-[var(--muted)] mt-1">
            하드코딩을 원천 배제하고 대한민국 노동/상법 법령 및 i18n 국제화 키를 중앙 집중 관리합니다. 코드 수정 없이 용어와 법적 근거를 통제합니다.
          </p>
        </div>

        <div className="flex items-center gap-3">
          <div className="flex items-center gap-1.5 p-1 rounded-lg border border-[var(--border)] bg-[var(--surface)] text-xs font-medium">
            <Globe2 className="w-3.5 h-3.5 text-[var(--muted)] ml-1" />
            <span className="text-[var(--muted)] mr-1">미리보기 언어:</span>
            <button
              onClick={() => setLocale("ko")}
              className={`px-2.5 py-1 rounded transition-colors ${
                locale === "ko"
                  ? "bg-[var(--brand)] text-white font-semibold shadow-xs"
                  : "text-[var(--muted)] hover:text-[var(--foreground)]"
              }`}
            >
              한국어 (KO)
            </button>
            <button
              onClick={() => setLocale("en")}
              className={`px-2.5 py-1 rounded transition-colors ${
                locale === "en"
                  ? "bg-[var(--brand)] text-white font-semibold shadow-xs"
                  : "text-[var(--muted)] hover:text-[var(--foreground)]"
              }`}
            >
              English (EN)
            </button>
          </div>
        </div>
      </div>

      {/* 2. Stat HUD */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
        <div className="p-4 rounded-xl border border-[var(--border)] bg-[var(--surface)]">
          <div className="text-xs text-[var(--muted)] font-medium">등록된 도메인 표준 용어</div>
          <div className="text-2xl font-bold font-mono text-[var(--foreground)] mt-1">
            {stats.total} <span className="text-xs font-normal text-[var(--muted)]">개 항목</span>
          </div>
          <div className="text-[11px] text-[var(--brand)] mt-1 flex items-center gap-1">
            <CheckCircle2 className="w-3.5 h-3.5" /> 100% i18n 이중 언어 매핑 완료
          </div>
        </div>

        <div className="p-4 rounded-xl border border-[var(--border)] bg-[var(--surface)]">
          <div className="text-xs text-[var(--muted)] font-medium">법률 및 시행령 연계 조항</div>
          <div className="text-2xl font-bold font-mono text-emerald-600 mt-1">
            {stats.withLaw} <span className="text-xs font-normal text-[var(--muted)]">개 법정 항목</span>
          </div>
          <div className="text-[11px] text-[var(--muted)] mt-1 flex items-center gap-1">
            <Scale className="w-3.5 h-3.5" /> 근로기준법, 국고금관리법, 산안법
          </div>
        </div>

        <div className="p-4 rounded-xl border border-[var(--border)] bg-[var(--surface)]">
          <div className="text-xs text-[var(--muted)] font-medium">관리 대상 업무 영역</div>
          <div className="text-2xl font-bold font-mono text-indigo-600 mt-1">
            {stats.categoriesCount} <span className="text-xs font-normal text-[var(--muted)]">개 도메인</span>
          </div>
          <div className="text-[11px] text-[var(--muted)] mt-1 flex items-center gap-1">
            <Layers className="w-3.5 h-3.5" /> HR, 근태, 급여, 운영, 인가, 감사
          </div>
        </div>
      </div>

      {/* 3. Search & Category Filters */}
      <div className="flex flex-col md:flex-row items-stretch md:items-center justify-between gap-3">
        <div className="relative flex-1 max-w-md">
          <Search className="w-4 h-4 text-[var(--muted)] absolute left-3 top-1/2 -translate-y-1/2" />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="용어명, 영문, 법률 조항(예: 제56조), 키 검색..."
            className="w-full h-9 pl-9 pr-3 rounded-lg border border-[var(--border)] bg-[var(--surface)] text-xs focus:outline-none focus:ring-2 focus:ring-[var(--brand)]"
          />
        </div>

        {/* Categories scroll rail */}
        <div className="flex items-center gap-1.5 overflow-x-auto pb-1 max-w-full">
          {Object.entries(CATEGORY_LABELS).map(([catKey, meta]) => {
            const isSelected = selectedCategory === catKey;
            return (
              <button
                key={catKey}
                onClick={() => setSelectedCategory(catKey)}
                className={`px-2.5 py-1.5 rounded-lg text-xs font-medium whitespace-nowrap transition-colors flex items-center gap-1.5 ${
                  isSelected
                    ? "bg-[var(--surface-active)] text-[var(--foreground)] border border-[var(--border)] font-semibold shadow-xs"
                    : "text-[var(--muted)] hover:text-[var(--foreground)] border border-transparent"
                }`}
              >
                <meta.icon className="w-3.5 h-3.5 opacity-70" />
                <span>{meta.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      {/* 4. Terminology Grid & Inspection Table */}
      <div className="rounded-xl border border-[var(--border)] bg-[var(--surface)] overflow-hidden shadow-xs">
        <div className="overflow-x-auto">
          <table className="w-full text-xs text-left border-collapse">
            <thead className="bg-[var(--surface-active)] border-b border-[var(--border)] text-[var(--muted)] font-semibold uppercase tracking-wider">
              <tr>
                <th className="py-2.5 px-3">식별자 키 (Key Path)</th>
                <th className="py-2.5 px-3">도메인 분류</th>
                <th className="py-2.5 px-3">한국어 표준 명칭 ({locale === "ko" ? "선택됨" : "원문"})</th>
                <th className="py-2.5 px-3">영문 표기 ({locale === "en" ? "선택됨" : "원문"})</th>
                <th className="py-2.5 px-3">법적 근거 (Statutory Basis)</th>
                <th className="py-2.5 px-3">설명 및 업무 규정</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border)]">
              {filteredTerms.map((entry) => {
                const isCurrentMatch = selectedEntry?.key === entry.key;
                return (
                  <tr
                    key={entry.key}
                    onClick={() => setSelectedEntry(entry)}
                    className={`hover:bg-[var(--surface-hover)] cursor-pointer transition-colors ${
                      isCurrentMatch ? "bg-[var(--brand-subtle)]/40" : ""
                    }`}
                  >
                    <td className="py-2.5 px-3 font-mono text-[11px] text-[var(--brand)] font-semibold">
                      {entry.key}
                    </td>
                    <td className="py-2.5 px-3 whitespace-nowrap">
                      <StatusChip
                        tone={CATEGORY_LABELS[entry.category]?.tone || "neutral"}
                        label={CATEGORY_LABELS[entry.category]?.label || entry.category}
                      />
                    </td>
                    <td className="py-2.5 px-3 font-semibold text-[var(--foreground)]">
                      {entry.ko}
                    </td>
                    <td className="py-2.5 px-3 text-[var(--muted)] font-medium">
                      {entry.en}
                    </td>
                    <td className="py-2.5 px-3 whitespace-nowrap">
                      {entry.legalBasis ? (
                        <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[11px] font-semibold bg-emerald-50 dark:bg-emerald-950/40 text-emerald-700 dark:text-emerald-400 border border-emerald-200 dark:border-emerald-800">
                          <Scale className="w-3 h-3" />
                          {entry.legalBasis}
                        </span>
                      ) : (
                        <span className="text-[var(--muted)] font-mono text-[11px]">-</span>
                      )}
                    </td>
                    <td className="py-2.5 px-3 text-[var(--muted)] max-w-md truncate">
                      {entry.description}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>

      {/* 5. Inspection Drawer / Detail Card when clicked */}
      {selectedEntry && (
        <div className="p-4 rounded-xl border border-[var(--border)] bg-[var(--surface)] space-y-3 animate-in fade-in duration-200">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-2">
            <div className="flex items-center gap-2">
              <BookOpen className="w-4 h-4 text-[var(--brand)]" />
              <span className="font-bold text-sm">상세 용어 명세: {selectedEntry.ko} ({selectedEntry.en})</span>
              <span className="font-mono text-xs px-2 py-0.5 rounded bg-[var(--surface-active)] text-[var(--muted)]">
                {selectedEntry.key}
              </span>
            </div>
            <Button size="sm" variant="ghost" onClick={() => setSelectedEntry(null)}>
              닫기
            </Button>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs">
            <div>
              <span className="text-[var(--muted)] font-semibold block mb-1">법적 근거 및 규정</span>
              <div className="p-2.5 rounded border border-[var(--border)] bg-[var(--background)] font-mono text-[11px]">
                {selectedEntry.legalBasis ? (
                  <span className="text-emerald-600 font-bold">{selectedEntry.legalBasis}</span>
                ) : (
                  <span className="text-[var(--muted)]">법률 직결 조항 없음 (내부 운영 규정)</span>
                )}
              </div>
            </div>

            <div>
              <span className="text-[var(--muted)] font-semibold block mb-1">실시간 치환 출력 테스트 (t function)</span>
              <div className="p-2.5 rounded border border-[var(--border)] bg-[var(--background)] font-mono text-[11px] text-[var(--brand)]">
                t(&quot;{selectedEntry.key}&quot;) =&gt; &quot;{t(selectedEntry.key)}&quot;
              </div>
            </div>
          </div>

          <div>
            <span className="text-[var(--muted)] font-semibold block mb-1">업무 및 시스템 영향도 설명</span>
            <p className="text-xs text-[var(--foreground)] leading-relaxed bg-[var(--background)] p-2.5 rounded border border-[var(--border)]">
              {selectedEntry.description}
            </p>
          </div>
        </div>
      )}
    </div>
  );
}
