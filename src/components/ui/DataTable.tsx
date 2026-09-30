"use client";

import React, { useState, useEffect, useRef } from "react";
import { clsx } from "clsx";
import { Search, ArrowUpDown, ClipboardPaste } from "lucide-react";

export interface Column<T> {
  key: string;
  header: string;
  width?: string;
  align?: "left" | "center" | "right";
  render?: (row: T, index: number) => React.ReactNode;
}

export interface DataTableProps<T> {
  columns: Column<T>[];
  data: T[];
  keyExtractor: (item: T) => string;
  onRowClick?: (item: T) => void;
  onPasteRows?: (rows: Record<string, string>[]) => void;
  searchPlaceholder?: string;
  searchFilter?: (item: T, query: string) => boolean;
  actions?: React.ReactNode;
  dense?: boolean;
}

export function DataTable<T>({
  columns,
  data,
  keyExtractor,
  onRowClick,
  onPasteRows,
  searchPlaceholder = "필터 검색...",
  searchFilter,
  actions,
  dense = true,
}: DataTableProps<T>) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState<number>(-1);
  const [pasteNotification, setPasteNotification] = useState<string | null>(null);
  const tableContainerRef = useRef<HTMLDivElement>(null);

  // 필터링 적용
  const filteredData = React.useMemo(() => {
    if (!query || !searchFilter) return data;
    return data.filter((item) => searchFilter(item, query));
  }, [data, query, searchFilter]);

  // Excel 클립보드 붙여넣기 인터셉트 (Tab-Separated Values TSV 파싱)
  useEffect(() => {
    const el = tableContainerRef.current;
    if (!el || !onPasteRows) return;

    function handlePaste(e: ClipboardEvent) {
      const text = e.clipboardData?.getData("text/plain");
      if (!text || !text.includes("\t")) return; // 단순 텍스트는 통과

      e.preventDefault();
      const lines = text.trim().split("\n");
      if (lines.length === 0) return;

      const parsedRows: Record<string, string>[] = [];
      lines.forEach((line) => {
        const cells = line.split("\t");
        const rowObj: Record<string, string> = {};
        columns.forEach((col, idx) => {
          if (cells[idx] !== undefined) {
            rowObj[col.key] = cells[idx].trim();
          }
        });
        parsedRows.push(rowObj);
      });

      if (onPasteRows && parsedRows.length > 0) {
        onPasteRows(parsedRows);
        setPasteNotification(`엑셀 클립보드 ${parsedRows.length}행이 성공적으로 파싱되어 적재되었습니다.`);
        setTimeout(() => setPasteNotification(null), 4000);
      }
    }

    el.addEventListener("paste", handlePaste);
    return () => el.removeEventListener("paste", handlePaste);
  }, [columns, onPasteRows]);

  // Vim / Google Workspace 스타일 키보드 탐색 (J: 아래, K: 위, Enter: 선택)
  useEffect(() => {
    const el = tableContainerRef.current;
    if (!el) return;

    function handleKeyDown(e: KeyboardEvent) {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return;
      }

      if (e.key.toLowerCase() === "j") {
        e.preventDefault();
        setSelectedIndex((prev) => Math.min(prev + 1, filteredData.length - 1));
      } else if (e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSelectedIndex((prev) => Math.max(prev - 1, 0));
      } else if (e.key === "Enter" && selectedIndex >= 0 && onRowClick) {
        e.preventDefault();
        const selectedItem = filteredData[selectedIndex];
        if (selectedItem) onRowClick(selectedItem);
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [filteredData, selectedIndex, onRowClick]);

  return (
    <div
      ref={tableContainerRef}
      tabIndex={0}
      className="flex flex-col rounded-[6px] border border-[var(--border)] bg-[var(--surface)] shadow-2xs focus:outline-none focus:ring-1 focus:ring-[var(--signal)] layout-contain-table"
    >
      {/* 툴바 / 검색 및 액션 영역 */}
      <div className="flex items-center justify-between gap-3 px-3 py-2 border-b border-[var(--border)] bg-gray-50/50">
        <div className="relative flex-1 max-w-xs">
          <Search className="absolute left-2.5 top-2 h-3.5 w-3.5 text-[var(--faint)]" />
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={searchPlaceholder}
            className="w-full pl-8 pr-3 py-1 text-xs rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] placeholder:text-[var(--faint)] focus:outline-none focus:border-[var(--steel)]"
          />
        </div>

        <div className="flex items-center gap-2">
          {onPasteRows && (
            <span className="hidden sm:inline-flex items-center gap-1 text-[11px] text-[var(--faint)] mr-1">
              <ClipboardPaste className="w-3 h-3" />
              <span>엑셀 복사-붙여넣기(Ctrl+V) 지원</span>
            </span>
          )}
          {actions}
        </div>
      </div>

      {/* 엑셀 붙여넣기 성공 알림 배너 */}
      {pasteNotification && (
        <div className="bg-emerald-50 px-3 py-1.5 border-b border-emerald-200 text-emerald-800 text-xs flex items-center justify-between animate-in fade-in duration-150">
          <span>✓ {pasteNotification}</span>
          <button
            onClick={() => setPasteNotification(null)}
            className="text-emerald-700 hover:text-emerald-900 font-bold ml-2 cursor-pointer"
          >
            ×
          </button>
        </div>
      )}

      {/* 테이블 본문 */}
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left text-xs">
          <thead>
            <tr className="border-b border-[var(--border)] bg-[var(--muted)] text-[var(--steel)] select-none">
              {columns.map((col) => (
                <th
                  key={col.key}
                  style={{ width: col.width }}
                  className={clsx(
                    "px-3 py-2 font-semibold text-[11px] tracking-tight",
                    col.align === "right" && "text-right",
                    col.align === "center" && "text-center"
                  )}
                >
                  <div className="inline-flex items-center gap-1">
                    <span>{col.header}</span>
                    <ArrowUpDown className="w-2.5 h-2.5 opacity-40" />
                  </div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-[var(--border-soft)]">
            {filteredData.length === 0 ? (
              <tr>
                <td colSpan={columns.length} className="px-4 py-8 text-center text-[var(--faint)]">
                  데이터가 존재하지 않습니다.
                </td>
              </tr>
            ) : (
              filteredData.map((row, index) => {
                const isSelected = selectedIndex === index;
                return (
                  <tr
                    key={keyExtractor(row)}
                    onClick={() => {
                      setSelectedIndex(index);
                      if (onRowClick) onRowClick(row);
                    }}
                    className={clsx(
                      "transition-colors cursor-pointer",
                      dense ? "h-8" : "h-10",
                      isSelected
                        ? "bg-amber-50/70 border-l-2 border-l-[var(--signal)]"
                        : "hover:bg-gray-50/80"
                    )}
                  >
                    {columns.map((col) => (
                      <td
                        key={col.key}
                        className={clsx(
                          "px-3 py-1.5 truncate text-[var(--ink)]",
                          col.align === "right" && "text-right font-mono",
                          col.align === "center" && "text-center"
                        )}
                      >
                        {col.render ? col.render(row, index) : String((row as any)[col.key] ?? "")}
                      </td>
                    ))}
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      {/* 테이블 하단 푸터 (건수 및 단축키 안내) */}
      <div className="flex items-center justify-between px-3 py-1.5 border-t border-[var(--border)] bg-gray-50/30 text-[11px] text-[var(--faint)]">
        <span>총 {filteredData.length}건</span>
        <span className="hidden sm:inline">단축키: J(아래) · K(위) · Enter(상세)</span>
      </div>
    </div>
  );
}
