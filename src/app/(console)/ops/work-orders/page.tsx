"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { DataTable, Column } from "@/components/ui/DataTable";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Button } from "@/components/ui/Button";
import { StatBar } from "@/components/ui/StatBar";
import { WorkOrder } from "@/lib/types";
import { ModuleScopeFilter, ModuleScopeState } from "@/components/gov/ModuleScopeFilter";
import {
  Wrench,
  Plus,
  ArrowRight,
  ArrowLeft,
  AlertCircle,
  Truck,
  CheckSquare,
  Clock,
  LayoutGrid,
  Kanban,
  X,
  ShieldCheck,
  FileText,
  MessageSquare,
  ChevronRight,
} from "lucide-react";

const STAGES: WorkOrder["status"][] = ["접수", "배차", "진행중", "완료", "검수"];

const STAGE_CONFIG: Record<
  WorkOrder["status"],
  { label: string; bg: string; border: string; text: string; dot: string }
> = {
  접수: {
    label: "1. 현장 접수",
    bg: "bg-slate-50",
    border: "border-slate-200",
    text: "text-slate-700",
    dot: "bg-slate-400",
  },
  배차: {
    label: "2. 기사 배차",
    bg: "bg-sky-50",
    border: "border-sky-200",
    text: "text-sky-800",
    dot: "bg-sky-500",
  },
  진행중: {
    label: "3. 정비 진행중",
    bg: "bg-amber-50",
    border: "border-amber-200",
    text: "text-amber-800",
    dot: "bg-amber-500",
  },
  완료: {
    label: "4. 현장 작업완료",
    bg: "bg-emerald-50",
    border: "border-emerald-200",
    text: "text-emerald-800",
    dot: "bg-emerald-500",
  },
  검수: {
    label: "5. 안전/품질 검수",
    bg: "bg-purple-50",
    border: "border-purple-200",
    text: "text-purple-800",
    dot: "bg-purple-500",
  },
};

export default function WorkOrdersPage() {
  const workOrders = useAppStore((s) => s.workOrders);
  const employees = useAppStore((s) => s.employees);
  const orgSites = useAppStore((s) => s.orgSites);
  const createWorkOrder = useAppStore((s) => s.createWorkOrder);
  const updateWorkOrderStatus = useAppStore((s) => s.updateWorkOrderStatus);
  const openObjectInspector = useAppStore((s) => s.openObjectInspector);
  const addToast = useAppStore((s) => s.addToast);

  const [viewMode, setViewMode] = useState<"kanban" | "grid">("kanban");
  const [creatorModalOpen, setCreatorModalOpen] = useState(false);
  const [selectedOrder, setSelectedOrder] = useState<WorkOrder | null>(null);

  // 모듈 관할 인가 범위 필터 상태
  const [scopeFilter, setScopeFilter] = useState<ModuleScopeState>({
    entityId: "all",
    siteId: "all",
    deptId: "all",
  });

  // 인가 범위 필터링된 오더 목록
  const filteredWorkOrders = React.useMemo(() => {
    return workOrders.filter((w) => {
      if (scopeFilter.siteId !== "all" && w.site !== scopeFilter.siteId) return false;
      return true;
    });
  }, [workOrders, scopeFilter]);

  // 안전 체크리스트 로컬 상태 (오더 ID별)
  const [safetyChecks, setSafetyChecks] = useState<Record<string, Record<string, boolean>>>({
    "wo-1": { ppe: true, loto: true, perimeter: true, ext: true, test: false },
    "wo-2": { ppe: true, loto: true, perimeter: false, ext: true, test: false },
  });

  // 작업오더 폼 필드
  const [title, setTitle] = useState("");
  const [site, setSite] = useState(orgSites[0]?.name || "");
  const [priority, setPriority] = useState<WorkOrder["priority"]>("보통");
  const [assignedTo, setAssignedTo] = useState(employees[0]?.name || "");
  const [targetEquipment, setTargetEquipment] = useState("");
  const [contractCode, setContractCode] = useState("C-207");
  const [dueDate, setDueDate] = useState("2026-07-06");

  const handleCreateSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim()) return;

    const created = createWorkOrder({
      title,
      site,
      priority,
      assignedTo,
      targetEquipment: targetEquipment || "공용 장비",
      dueDate,
      contractCode,
    });

    setCreatorModalOpen(false);
    setSelectedOrder(created);
    setTitle("");
  };

  const getNextStatus = (current: WorkOrder["status"]): WorkOrder["status"] | null => {
    const idx = STAGES.indexOf(current);
    if (idx >= 0 && idx < STAGES.length - 1) {
      return STAGES[idx + 1];
    }
    return null;
  };

  const getPrevStatus = (current: WorkOrder["status"]): WorkOrder["status"] | null => {
    const idx = STAGES.indexOf(current);
    if (idx > 0) {
      return STAGES[idx - 1];
    }
    return null;
  };

  const toggleSafetyCheck = (orderId: string, checkKey: string) => {
    setSafetyChecks((prev) => {
      const orderChecks = prev[orderId] || {};
      const updated = { ...orderChecks, [checkKey]: !orderChecks[checkKey] };
      return { ...prev, [orderId]: updated };
    });
    addToast({
      title: "안전 점검 항목 업데이트",
      description: "작업 안전 규정 준수 체크리스트가 갱신되었습니다.",
      tone: "info",
    });
  };

  const columns: Column<WorkOrder>[] = [
    {
      key: "code",
      header: "오더 번호",
      width: "110px",
      render: (row) => <ObjectLink code={row.code} />,
    },
    {
      key: "priority",
      header: "우선순위",
      width: "80px",
      render: (row) => (
        <StatusChip
          label={row.priority}
          tone={row.priority === "긴급" ? "danger" : row.priority === "높음" ? "warn" : "info"}
          size="xs"
        />
      ),
    },
    {
      key: "title",
      header: "작업 및 정비 내용",
      render: (row) => (
        <span className="font-semibold text-[var(--ink)] block truncate max-w-sm">
          {row.title}
        </span>
      ),
    },
    {
      key: "site",
      header: "현장",
      render: (row) => <span className="text-[var(--steel)]">{row.site}</span>,
    },
    {
      key: "targetEquipment",
      header: "대상 장비",
      width: "130px",
      render: (row) => <span className="font-mono text-[11px]">{row.targetEquipment || "-"}</span>,
    },
    {
      key: "assignedTo",
      header: "담당 엔지니어",
      width: "100px",
      render: (row) => <span className="font-medium">{row.assignedTo}</span>,
    },
    {
      key: "status",
      header: "작업 공정",
      width: "90px",
      render: (row) => <StatusChip label={row.status} size="xs" dot />,
    },
    {
      key: "action",
      header: "공정 제어",
      width: "120px",
      render: (row) => {
        const next = getNextStatus(row.status);
        return next ? (
          <Button
            size="xs"
            variant={row.priority === "긴급" ? "brand" : "secondary"}
            onClick={() => updateWorkOrderStatus(row.id, next)}
          >
            {next}로 전이 →
          </Button>
        ) : (
          <span className="text-[10px] text-purple-700 font-bold font-mono">검수 완료</span>
        );
      },
    },
  ];

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "전체 작업오더", value: `${workOrders.length}건` },
          {
            label: "긴급 수리 건",
            value: `${workOrders.filter((w) => w.priority === "긴급").length}건`,
            badge: "현장 대기",
            badgeTone: "danger",
          },
          {
            label: "정비 진행중",
            value: `${workOrders.filter((w) => w.status === "진행중").length}건`,
            badgeTone: "warn",
          },
          {
            label: "검수 완료율",
            value: `${Math.round(
              (workOrders.filter((w) => w.status === "완료" || w.status === "검수").length /
                Math.max(1, workOrders.length)) *
                100
            )}%`,
            badgeTone: "ok",
          },
        ]}
      />

      {/* 2. 상단 액션 바 및 뷰 전환 탭 */}
      <div className="flex items-center justify-between flex-wrap gap-3">
        <div className="flex items-center gap-2">
          {/* 뷰 모드 토글 */}
          <div className="inline-flex rounded-md border border-[var(--border)] bg-[var(--muted)] p-0.5 text-xs">
            <button
              onClick={() => setViewMode("kanban")}
              className={`flex items-center gap-1.5 px-3 py-1 rounded font-medium transition-colors cursor-pointer ${
                viewMode === "kanban"
                  ? "bg-[var(--surface)] text-[var(--ink)] shadow-2xs font-semibold"
                  : "text-[var(--steel)] hover:text-[var(--ink)]"
              }`}
            >
              <Kanban className="w-3.5 h-3.5" />
              <span>공정 칸반 보드</span>
            </button>
            <button
              onClick={() => setViewMode("grid")}
              className={`flex items-center gap-1.5 px-3 py-1 rounded font-medium transition-colors cursor-pointer ${
                viewMode === "grid"
                  ? "bg-[var(--surface)] text-[var(--ink)] shadow-2xs font-semibold"
                  : "text-[var(--steel)] hover:text-[var(--ink)]"
              }`}
            >
              <LayoutGrid className="w-3.5 h-3.5" />
              <span>그리드 대장</span>
            </button>
          </div>
          <span className="text-xs text-[var(--steel)] hidden md:inline">
            현장 고장 접수 시 즉시 작업오더(WO-)를 발행하고 정비 엔지니어에게 배차할 수 있습니다.
          </span>
        </div>

        <Button
          size="sm"
          variant="brand"
          leftIcon={<Plus className="w-3.5 h-3.5" />}
          onClick={() => setCreatorModalOpen(true)}
        >
          현장 작업오더 발행
        </Button>
      </div>

      {/* 2.5 모듈 레벨 관할 인가 범위 필터 */}
      <ModuleScopeFilter
        value={scopeFilter}
        onChange={setScopeFilter}
        filteredCount={filteredWorkOrders.length}
        totalCount={workOrders.length}
        showDept={false}
      />

      {/* 3. 칸반 뷰 또는 그리드 뷰 */}
      {viewMode === "kanban" ? (
        <div className="grid grid-cols-1 md:grid-cols-3 lg:grid-cols-5 gap-3.5 items-start">
          {STAGES.map((stage) => {
            const stageOrders = filteredWorkOrders.filter((w) => w.status === stage);
            const cfg = STAGE_CONFIG[stage];

            return (
              <div
                key={stage}
                className="rounded-lg border border-[var(--border)] bg-[var(--surface)] flex flex-col min-h-[460px] shadow-2xs layout-contain-column"
              >
                {/* 열 헤더 */}
                <div
                  className={`flex items-center justify-between px-3 py-2.5 border-b ${cfg.border} ${cfg.bg} rounded-t-lg`}
                >
                  <div className="flex items-center gap-2">
                    <span className={`w-2 h-2 rounded-full ${cfg.dot}`} />
                    <span className={`text-xs font-bold ${cfg.text}`}>{cfg.label}</span>
                  </div>
                  <span className="px-1.5 py-0.5 rounded-full bg-white/80 border border-[var(--border)] text-[10px] font-mono font-bold text-[var(--ink)]">
                    {stageOrders.length}
                  </span>
                </div>

                {/* 열 카드 목록 */}
                <div className="p-2.5 flex-1 space-y-2.5 overflow-y-auto max-h-[calc(100vh-280px)]">
                  {stageOrders.length === 0 ? (
                    <div className="h-32 flex flex-col items-center justify-center border-2 border-dashed border-[var(--border)] rounded-md text-[var(--faint)] text-xs">
                      <span>대기 오더 없음</span>
                    </div>
                  ) : (
                    stageOrders.map((wo) => {
                      const next = getNextStatus(wo.status);
                      const prev = getPrevStatus(wo.status);

                      return (
                        <div
                          key={wo.id}
                          onClick={() => setSelectedOrder(wo)}
                          className="group relative rounded-md border border-[var(--border)] bg-[var(--surface)] p-3 shadow-2xs hover:border-[var(--brand)] hover:shadow-sm transition-all cursor-pointer space-y-2"
                        >
                          {/* 카드 헤더: 번호 & 우선순위 */}
                          <div className="flex items-center justify-between text-xs">
                            <ObjectLink code={wo.code} />
                            <StatusChip
                              label={wo.priority}
                              tone={
                                wo.priority === "긴급"
                                  ? "danger"
                                  : wo.priority === "높음"
                                  ? "warn"
                                  : "info"
                              }
                              size="xs"
                            />
                          </div>

                          {/* 제목 */}
                          <h4 className="text-xs font-bold text-[var(--ink)] line-clamp-2 leading-relaxed">
                            {wo.title}
                          </h4>

                          {/* 메타데이터 */}
                          <div className="space-y-1 text-[11px] text-[var(--steel)] pt-1 border-t border-[var(--border)]/60">
                            <div className="flex items-center justify-between">
                              <span className="truncate">{wo.site}</span>
                              <span className="font-mono text-[10px] text-[var(--faint)]">
                                {wo.dueDate}
                              </span>
                            </div>
                            <div className="flex items-center justify-between">
                              <span className="font-medium text-[var(--ink)]">{wo.assignedTo}</span>
                              <span className="font-mono text-[10px] truncate max-w-[100px]">
                                {wo.targetEquipment}
                              </span>
                            </div>
                          </div>

                          {/* 1-클릭 공정 제어 버튼 */}
                          <div
                            className="flex items-center justify-between pt-2 border-t border-[var(--border)]/60 gap-1"
                            onClick={(e) => e.stopPropagation()}
                          >
                            {prev ? (
                              <button
                                onClick={() => updateWorkOrderStatus(wo.id, prev)}
                                className="px-1.5 py-0.5 rounded text-[10px] text-[var(--steel)] hover:bg-[var(--muted)] border border-transparent hover:border-[var(--border)] flex items-center gap-0.5 cursor-pointer"
                                title={`${prev} 단계로 회송`}
                              >
                                <ArrowLeft className="w-3 h-3" />
                                <span>{prev}</span>
                              </button>
                            ) : (
                              <div />
                            )}

                            {next ? (
                              <Button
                                size="xs"
                                variant={wo.priority === "긴급" ? "brand" : "secondary"}
                                onClick={() => updateWorkOrderStatus(wo.id, next)}
                              >
                                {next} →
                              </Button>
                            ) : (
                              <span className="text-[10px] text-purple-700 font-bold font-mono">
                                검수완료
                              </span>
                            )}
                          </div>
                        </div>
                      );
                    })
                  )}
                </div>
              </div>
            );
          })}
        </div>
      ) : (
        /* 그리드 대장 뷰 */
        <DataTable
          columns={columns}
          data={filteredWorkOrders}
          keyExtractor={(item) => item.id}
          onRowClick={(item) => setSelectedOrder(item)}
          searchPlaceholder="오더번호, 정비내용, 장비, 현장 검색..."
          searchFilter={(item, q) =>
            item.title.toLowerCase().includes(q.toLowerCase()) ||
            item.code.toLowerCase().includes(q.toLowerCase()) ||
            item.site.toLowerCase().includes(q.toLowerCase()) ||
            item.assignedTo.toLowerCase().includes(q.toLowerCase())
          }
        />
      )}

      {/* 4. 작업오더 상세 360 드로어 */}
      {selectedOrder && (
        <div className="fixed inset-0 z-50 flex justify-end bg-black/40 backdrop-blur-2xs">
          <div className="w-full max-w-md h-full bg-[var(--surface)] border-l border-[var(--border)] shadow-2xl flex flex-col animate-in slide-in-from-right duration-200">
            {/* 드로어 헤더 */}
            <div className="p-4 border-b border-[var(--border)] flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Wrench className="w-5 h-5 text-amber-600" />
                <div>
                  <div className="flex items-center gap-2">
                    <span className="font-mono font-bold text-sm text-[var(--ink)]">
                      {selectedOrder.code}
                    </span>
                    <StatusChip label={selectedOrder.status} size="xs" dot />
                  </div>
                  <span className="text-[11px] text-[var(--steel)]">현장 정비 원장 상세</span>
                </div>
              </div>
              <button
                onClick={() => setSelectedOrder(null)}
                className="p-1 rounded hover:bg-[var(--muted)] text-[var(--steel)] hover:text-[var(--ink)] cursor-pointer"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            {/* 드로어 본문 스크롤 영역 */}
            <div className="flex-1 overflow-y-auto p-4 space-y-5 text-xs">
              {/* 기본 정보 카드 */}
              <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-3.5 space-y-3">
                <div className="flex items-center justify-between">
                  <span className="text-[11px] font-semibold text-[var(--steel)]">오더 제목</span>
                  <StatusChip
                    label={selectedOrder.priority}
                    tone={
                      selectedOrder.priority === "긴급"
                        ? "danger"
                        : selectedOrder.priority === "높음"
                        ? "warn"
                        : "info"
                    }
                    size="xs"
                  />
                </div>
                <div className="text-sm font-bold text-[var(--ink)] leading-snug">
                  {selectedOrder.title}
                </div>

                <div className="grid grid-cols-2 gap-2 pt-2 border-t border-[var(--border)] text-[11px]">
                  <div>
                    <span className="text-[var(--steel)] block">발생 현장</span>
                    <span className="font-medium text-[var(--ink)]">{selectedOrder.site}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">배차 엔지니어</span>
                    <span className="font-medium text-[var(--ink)]">{selectedOrder.assignedTo}</span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">대상 장비 / 차량</span>
                    <span className="font-mono text-[var(--ink)]">
                      {selectedOrder.targetEquipment || "-"}
                    </span>
                  </div>
                  <div>
                    <span className="text-[var(--steel)] block">완료 요구 기한</span>
                    <span className="font-mono text-[var(--ink)]">{selectedOrder.dueDate}</span>
                  </div>
                </div>
              </div>

              {/* LSA / 산업안전보건법 작업 전 현장 안전 체크리스트 */}
              <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-3.5 space-y-2.5">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-1.5 font-bold text-[var(--ink)]">
                    <ShieldCheck className="w-4 h-4 text-emerald-600" />
                    <span>산업안전보건법 필수 준수 점검표</span>
                  </div>
                  <span className="text-[10px] text-emerald-700 font-mono bg-emerald-50 border border-emerald-200 px-1.5 py-0.5 rounded">
                    안전우선
                  </span>
                </div>
                <div className="space-y-2 text-[11px]">
                  {[
                    {
                      id: "ppe",
                      label: "개인보호구(안전모·보안경·절연화) 착용 확인",
                    },
                    {
                      id: "loto",
                      label: "장비 전원 차단 및 LOTO (Lock-Out / Tag-Out) 표지판 부착",
                    },
                    {
                      id: "perimeter",
                      label: "작업 반경 5m 안전 라바콘 및 접근 통제선 설치",
                    },
                    {
                      id: "ext",
                      label: "소화기 비치 및 주변 인화성 물질 격리 확인",
                    },
                    {
                      id: "test",
                      label: "작업 종료 후 누유/누설 검사 및 시운전 정상 확인",
                    },
                  ].map((chk) => {
                    const isChecked = !!safetyChecks[selectedOrder.id]?.[chk.id];
                    return (
                      <label
                        key={chk.id}
                        className="flex items-start gap-2 p-1.5 rounded hover:bg-[var(--muted)] cursor-pointer"
                      >
                        <input
                          type="checkbox"
                          checked={isChecked}
                          onChange={() => toggleSafetyCheck(selectedOrder.id, chk.id)}
                          className="mt-0.5 rounded border-gray-300 text-[var(--brand)] focus:ring-0"
                        />
                        <span className={isChecked ? "text-[var(--ink)] font-medium" : "text-[var(--steel)]"}>
                          {chk.label}
                        </span>
                      </label>
                    );
                  })}
                </div>
              </div>

              {/* 연계 구매 품의 / 도급 계약 정보 */}
              <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-3.5 space-y-2.5">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-1.5 font-bold text-[var(--ink)]">
                    <FileText className="w-4 h-4 text-[var(--brand)]" />
                    <span>연계 엔티티 & 도급계약</span>
                  </div>
                </div>
                <div className="space-y-2 text-[11px]">
                  <div className="flex items-center justify-between p-2 rounded bg-[var(--muted)]">
                    <span className="text-[var(--steel)]">도급 계약 코드</span>
                    <ObjectLink code={selectedOrder.contractCode || "C-207"} />
                  </div>
                  <div className="flex items-center justify-between p-2 rounded bg-[var(--muted)]">
                    <span className="text-[var(--steel)]">정비 부품 구매품의 (DOA)</span>
                    <ObjectLink code="AP-3121" />
                  </div>
                </div>
              </div>

              {/* 엔티티 디스커션 스레드 연결 */}
              <div className="rounded-lg border border-[var(--border)] bg-sky-50/50 p-3 flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <MessageSquare className="w-4 h-4 text-sky-700" />
                  <div>
                    <div className="font-semibold text-sky-900">현장 실시간 스레드</div>
                    <div className="text-[10px] text-sky-700">이 작업오더와 연계된 메신저 채널</div>
                  </div>
                </div>
                <Button
                  size="xs"
                  variant="secondary"
                  onClick={() => openObjectInspector(selectedOrder.code)}
                >
                  스레드 열기
                </Button>
              </div>
            </div>

            {/* 드로어 하단 공정 제어 푸터 */}
            <div className="p-4 border-t border-[var(--border)] bg-[var(--canvas)] flex items-center justify-between gap-2">
              <div className="text-xs text-[var(--steel)]">
                현재 공정: <span className="font-bold text-[var(--ink)]">{selectedOrder.status}</span>
              </div>
              <div className="flex items-center gap-2">
                {getNextStatus(selectedOrder.status) && (
                  <Button
                    size="sm"
                    variant="brand"
                    onClick={() => {
                      const next = getNextStatus(selectedOrder.status);
                      if (next) {
                        updateWorkOrderStatus(selectedOrder.id, next);
                        setSelectedOrder({ ...selectedOrder, status: next });
                      }
                    }}
                  >
                    {getNextStatus(selectedOrder.status)} 단계로 전이 →
                  </Button>
                )}
                <Button size="sm" variant="ghost" onClick={() => setSelectedOrder(null)}>
                  닫기
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* 5. 신규 작업오더 발행 모달 */}
      {creatorModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Wrench className="w-5 h-5 text-amber-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 현장 작업오더 (WO-) 발행</h3>
              </div>
              <button
                onClick={() => setCreatorModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer"
              >
                ×
              </button>
            </div>

            <form onSubmit={handleCreateSubmit} className="space-y-3 text-xs">
              <div>
                <label className="font-semibold block mb-1">오더 제목 / 정비 내용 *</label>
                <input
                  type="text"
                  required
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  placeholder="예: 평택 항만 40톤 트랙터 유압 호스 긴급 교체"
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">발생 현장</label>
                  <select
                    value={site}
                    onChange={(e) => setSite(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgSites.map((st) => (
                      <option key={st.id} value={st.name}>
                        {st.name}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">우선순위</label>
                  <select
                    value={priority}
                    onChange={(e) => setPriority(e.target.value as any)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="보통">보통 (일반정비)</option>
                    <option value="높음">높음 (당일배차)</option>
                    <option value="긴급">긴급 (현장라인 정지)</option>
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">대상 장비 / 차량</label>
                  <input
                    type="text"
                    value={targetEquipment}
                    onChange={(e) => setTargetEquipment(e.target.value)}
                    placeholder="예: 지게차 7톤 (FL-07-당진)"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">담당 배차 기사</label>
                  <select
                    value={assignedTo}
                    onChange={(e) => setAssignedTo(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {employees.map((emp) => (
                      <option key={emp.id} value={emp.name}>
                        {emp.name} ({emp.dept} · {emp.position})
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">연계 도급계약 (C-)</label>
                  <input
                    type="text"
                    value={contractCode}
                    onChange={(e) => setContractCode(e.target.value)}
                    placeholder="C-207"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">완료 요구 기한</label>
                  <input
                    type="date"
                    value={dueDate}
                    onChange={(e) => setDueDate(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button variant="ghost" size="sm" onClick={() => setCreatorModalOpen(false)}>
                  취소
                </Button>
                <Button variant="brand" size="sm" type="submit">
                  작업오더 발행
                </Button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}
