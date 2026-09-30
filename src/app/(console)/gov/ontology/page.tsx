"use client";

import React, { useState } from "react";
import Link from "next/link";
import { useAppStore } from "@/lib/store";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { StatusChip } from "@/components/ui/StatusChip";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import {
  Layers,
  ArrowRight,
  Database,
  GitBranch,
  Shield,
  Activity,
  Cpu,
  Search,
  ExternalLink,
  PlusCircle,
  Eye,
  CheckCircle2,
} from "lucide-react";

interface OntologySchemaType {
  code: string;
  name: string;
  prefix: string;
  steward: string;
  description: string;
  layer: "Semantic (의미)" | "Kinetic (동작)" | "Dynamic (역학)";
  createUrl: string;
  createLabel: string;
}

const SCHEMAS: OntologySchemaType[] = [
  {
    code: "OT-01",
    name: "도급 및 수주 계약",
    prefix: "C-",
    steward: "경영기획실",
    description: "현장별 도급·용역 계약. 인력 편성(TO) 및 원가 산정의 상류 기원.",
    layer: "Semantic (의미)",
    createUrl: "/ops/setup",
    createLabel: "신규 계약 등록",
  },
  {
    code: "OT-02",
    name: "임직원 및 작업자",
    prefix: "EMP-",
    steward: "인사노무팀",
    description: "전사 및 현장 파견 인력의 신원, 포지션, 통상임금 및 Cedar 주체(Principal).",
    layer: "Semantic (의미)",
    createUrl: "/hr/people",
    createLabel: "신규 사원 입사 등록",
  },
  {
    code: "OT-03",
    name: "근태 및 실적 펀치",
    prefix: "AT-",
    steward: "인사노무팀",
    description: "일별 계획 대 실적 펀치. 주52시간 한도 및 법정 휴게시간(근기법 §54) 산정.",
    layer: "Kinetic (동작)",
    createUrl: "/hr/attendance",
    createLabel: "근태 예외 심사/펀치 보정",
  },
  {
    code: "OT-04",
    name: "전자결재 및 인가",
    prefix: "AP-",
    steward: "경영지원팀",
    description: "단일 거버넌스 쓰기 경로. SAP Document Parking 및 다단계 결재선.",
    layer: "Kinetic (동작)",
    createUrl: "/approvals",
    createLabel: "신규 결재 기안 상신",
  },
  {
    code: "OT-05",
    name: "현장 정비 및 작업오더",
    prefix: "WO-",
    steward: "운영정비본부",
    description: "현장 장비 고장 접수, 배차, 엔지니어 정비 이력 및 원가 연계.",
    layer: "Kinetic (동작)",
    createUrl: "/ops/work-orders",
    createLabel: "신규 작업오더 발행",
  },
  {
    code: "OT-06",
    name: "법정 급여 및 명세서",
    prefix: "PS-",
    steward: "인사노무팀",
    description: "4대보험, 10원절사, 간이세액표 및 통상시급 기준 법정 임금 산출물.",
    layer: "Dynamic (역학)",
    createUrl: "/hr/payroll",
    createLabel: "정기 급여 계산 및 마감",
  },
  {
    code: "OT-07",
    name: "감사 이벤트 스트림",
    prefix: "AU-",
    steward: "감사팀",
    description: "SHA-256 해시체인 기반 불변 시계열 감사 원장. WORM 표준 준용.",
    layer: "Dynamic (역학)",
    createUrl: "/gov/audit",
    createLabel: "WORM 감사 원장 열람",
  },
];

interface LiveInstanceRow {
  code: string;
  title: string;
  subtext: string;
  status: string;
  statusTone: "ok" | "warn" | "danger" | "info" | "neutral";
  amountOrMetric?: string;
}

export default function OntologyPage() {
  const [selectedSchema, setSelectedSchema] = useState<OntologySchemaType>(SCHEMAS[0] || SCHEMAS[0]);
  const [instanceSearch, setInstanceSearch] = useState("");

  const contracts = useAppStore((s) => s.contracts);
  const employees = useAppStore((s) => s.employees);
  const attendance = useAppStore((s) => s.attendance);
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const payslips = useAppStore((s) => s.payslips);
  const auditEvents = useAppStore((s) => s.auditEvents);
  const openObjectInspector = useAppStore((s) => s.openObjectInspector);

  // 선택된 스키마에 따라 실시간 인스턴스 배열 동적 매핑
  const getLiveInstances = (schemaCode: string): LiveInstanceRow[] => {
    switch (schemaCode) {
      case "OT-01":
        return contracts.map((c) => ({
          code: c.code,
          title: c.title,
          subtext: `${c.client} · ${c.site}`,
          status: c.status,
          statusTone: c.status === "유효" ? "ok" : "neutral",
          amountOrMetric: `월 ₩${(c.monthlyAmount / 10000).toLocaleString()}만원`,
        }));
      case "OT-02":
        return employees.map((e) => ({
          code: e.id,
          title: e.name,
          subtext: `${e.dept} · ${e.role} (${e.site})`,
          status: e.status,
          statusTone: e.status === "재직" ? "ok" : "warn",
          amountOrMetric: `기본급 ₩${(e.baseSalary / 10000).toLocaleString()}만원`,
        }));
      case "OT-03":
        return attendance.map((a) => ({
          code: a.exceptionCode || a.id,
          title: `${a.employeeName} 근태 실적`,
          subtext: `${a.date} (${a.site}) · 실근로 ${a.workedHours}h`,
          status: a.status,
          statusTone: a.status === "정상" ? "ok" : a.status === "연장" ? "warn" : "danger",
          amountOrMetric: `주누적 ${a.weeklyHoursTotal}h`,
        }));
      case "OT-04":
        return approvals.map((a) => ({
          code: a.code,
          title: a.title,
          subtext: `기안: ${a.drafterName} (${a.drafterDept}) · ${a.category}`,
          status: a.status,
          statusTone:
            a.status === "승인완료"
              ? "ok"
              : a.status === "결재대기"
              ? "warn"
              : a.status === "반려"
              ? "danger"
              : "neutral",
          amountOrMetric: a.doaAmount ? `품의 ₩${(a.doaAmount / 10000).toLocaleString()}만원` : "-",
        }));
      case "OT-05":
        return workOrders.map((w) => ({
          code: w.code,
          title: w.title,
          subtext: `${w.site} · 담당: ${w.assignedTo}`,
          status: w.status,
          statusTone:
            w.status === "완료" || w.status === "검수"
              ? "ok"
              : w.status === "진행중"
              ? "warn"
              : "info",
          amountOrMetric: `우선순위: ${w.priority}`,
        }));
      case "OT-06":
        return payslips.map((p) => ({
          code: p.code || p.id,
          title: `${p.employeeName} 급여명세서`,
          subtext: `${p.yearMonth} 정기 지급분 (${p.empType})`,
          status: "확정",
          statusTone: "ok",
          amountOrMetric: `실지급 ₩${p.netPay.toLocaleString()}원`,
        }));
      case "OT-07":
        return auditEvents.map((au) => ({
          code: au.id,
          title: au.action.replace(/_/g, " "),
          subtext: `${au.actorName} · ${au.timestamp}`,
          status: "WORM 불변",
          statusTone: "ok",
          amountOrMetric: `해시 #${au.seq}`,
        }));
      default:
        return [];
    }
  };

  const currentInstances = getLiveInstances(selectedSchema.code);
  const filteredInstances = currentInstances.filter((inst) => {
    if (!instanceSearch.trim()) return true;
    const q = instanceSearch.toLowerCase();
    return (
      inst.code.toLowerCase().includes(q) ||
      inst.title.toLowerCase().includes(q) ||
      inst.subtext.toLowerCase().includes(q)
    );
  });

  const totalRegisteredEntities =
    contracts.length +
    employees.length +
    attendance.length +
    approvals.length +
    workOrders.length +
    payslips.length +
    auditEvents.length;

  return (
    <div className="space-y-5">
      {/* 1. 상단 스탯 바 */}
      <StatBar
        items={[
          {
            label: "등록 개체 타입",
            value: "7종 카탈로그",
            badge: "Palantir 동급",
            badgeTone: "info",
          },
          {
            label: "실시간 활성 인스턴스",
            value: `${totalRegisteredEntities}건`,
            subValue: "전사 원장 완전 동기화",
            badgeTone: "ok",
          },
          {
            label: "단일 쓰기 규율",
            value: "Action as Only Write Path",
            badgeTone: "ok",
          },
          {
            label: "Cedar PBAC 정책",
            value: "실시간 권한 폴드 연계",
            badgeTone: "info",
          },
        ]}
      />

      {/* 2. Palantir 3계층 온톨로지 설명 배너 */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
        <div className="p-3 rounded-lg border border-blue-200 bg-blue-50/60 space-y-1">
          <div className="flex items-center gap-1.5 font-bold text-blue-950">
            <Database className="w-3.5 h-3.5 text-blue-700" />
            <span>1. 의미 계층 (Semantic)</span>
          </div>
          <p className="text-[11px] text-blue-900 leading-relaxed">
            개체가 <strong>무엇인가</strong>: 계약(C-), 직원(EMP-), 조직, 사업장 등 명사형 불변 개체.
          </p>
        </div>

        <div className="p-3 rounded-lg border border-amber-200 bg-amber-50/60 space-y-1">
          <div className="flex items-center gap-1.5 font-bold text-amber-950">
            <Activity className="w-3.5 h-3.5 text-amber-700" />
            <span>2. 동작 계층 (Kinetic)</span>
          </div>
          <p className="text-[11px] text-amber-900 leading-relaxed">
            <strong>무슨 일이 일어나는가</strong>: 결재(AP-), 작업오더(WO-), 근태예외(AT-) 등 사건·상태 전이.
          </p>
        </div>

        <div className="p-3 rounded-lg border border-purple-200 bg-purple-50/60 space-y-1">
          <div className="flex items-center gap-1.5 font-bold text-purple-950">
            <Cpu className="w-3.5 h-3.5 text-purple-700" />
            <span>3. 역학 계층 (Dynamic)</span>
          </div>
          <p className="text-[11px] text-purple-900 leading-relaxed">
            <strong>시간에 따라 어떻게 계산·반응하는가</strong>: Cedar 정책, 급여 엔진(PS-), WORM 감사 원장.
          </p>
        </div>
      </div>

      {/* 2.5 왜 팔란티어식 개체(Entity) 엔진인가? */}
      <div className="p-4 rounded-lg border border-amber-200 bg-amber-50/40 space-y-3 text-xs">
        <div className="flex items-center gap-2 text-amber-950 font-bold">
          <Layers className="w-4 h-4 text-amber-700" />
          <span>엔터프라이즈 아키텍처 원칙: 왜 전통적 관계형 DB CRUD 대신 팔란티어식 개체 엔진인가?</span>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-[11px] text-[var(--ink)]">
          <div className="p-3 rounded-md bg-white border border-amber-200/70 space-y-1">
            <span className="font-bold text-amber-900 block">1. 사일로화된 CRUD 극복</span>
            <p className="text-[var(--steel)] leading-relaxed">
              기존 B2B SaaS는 인사, 회계, 정비, 결재가 서로 다른 DB 테이블에 갇혀 있어 사용자가 5개의 창을 오가야 했습니다. 개체 엔진은 모든 물리적 대상(모터, 기사, 품의서)을 상태와 링크를 가진 단일 개체 그래프로 연결합니다.
            </p>
          </div>
          <div className="p-3 rounded-md bg-white border border-amber-200/70 space-y-1">
            <span className="font-bold text-amber-900 block">2. 게임식 동적 연결 (Dynamic Linking)</span>
            <p className="text-[var(--steel)] leading-relaxed">
              게임 채팅에서 <code className="bg-gray-100 px-1 py-0.5 rounded font-mono">[아이템]</code>을 링크하면 실시간 능력치 카드와 거래 버튼이 뜨듯, 메신저나 결재문서에서 <code className="bg-gray-100 px-1 py-0.5 rounded font-mono">[WO-2641]</code>을 언급하면 실시간 미니카드와 1-클릭 승인/배차 액션이 즉시 활성화됩니다.
            </p>
          </div>
          <div className="p-3 rounded-md bg-white border border-amber-200/70 space-y-1">
            <span className="font-bold text-amber-900 block">3. 일상 원장과 기술 로그의 의도적 분리</span>
            <p className="text-[var(--steel)] leading-relaxed">
              사용자의 일반적인 일상 경로에서는 복잡한 암호학적 해시나 기술 정책 AST가 시야를 가리지 않고 오직 &apos;누가, 언제, 무엇을, 왜 처리했는가&apos;라는 업무 수행 원장만이 서사적으로 전달되며, 기술 로그는 상세 검증 경로로 분리됩니다.
            </p>
          </div>
        </div>
      </div>

      {/* 3. 개체 카탈로그 & 실시간 인스턴스 탐색기 */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
        {/* 개체 타입 목록 (4열) */}
        <div className="lg:col-span-4 rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs overflow-hidden">
          <div className="p-3 border-b border-[var(--border)] bg-gray-50/50 font-bold text-xs text-[var(--ink)] flex items-center justify-between">
            <span>개체 스키마 카탈로그</span>
            <span className="text-[10px] text-[var(--faint)] font-mono">7 Types</span>
          </div>
          <div className="divide-y divide-[var(--border-soft)]">
            {SCHEMAS.map((s) => {
              const count = getLiveInstances(s.code).length;
              return (
                <div
                  key={s.code}
                  onClick={() => {
                    setSelectedSchema(s);
                    setInstanceSearch("");
                  }}
                  className={`p-3 cursor-pointer transition-colors space-y-1 text-xs ${
                    selectedSchema.code === s.code
                      ? "bg-amber-50/80 border-l-2 border-l-[var(--signal)]"
                      : "hover:bg-gray-50"
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <span className="font-mono font-bold text-amber-900 bg-amber-100 px-1.5 py-0.2 rounded text-[10px]">
                        {s.prefix}
                      </span>
                      <span className="font-semibold text-[var(--ink)]">{s.name}</span>
                    </div>
                    <span className="font-mono text-[10px] bg-slate-100 text-slate-700 px-1.5 py-0.2 rounded">
                      {count}건
                    </span>
                  </div>
                  <p className="text-[11px] text-[var(--steel)] line-clamp-1">{s.description}</p>
                  <div className="flex items-center justify-between pt-1">
                    <span className="text-[10px] text-blue-700 font-medium">{s.layer}</span>
                    <span className="text-[10px] text-[var(--faint)]">관리: {s.steward}</span>
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        {/* 선택된 개체의 실시간 인스턴스 탐색기 (8열) */}
        <div className="lg:col-span-8 space-y-4">
          <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4 text-xs">
            {/* 스키마 상단 메타데이터 및 신규 등록 액션 */}
            <div className="border-b border-[var(--border)] pb-3 flex flex-wrap items-start justify-between gap-3">
              <div>
                <div className="flex items-center gap-2">
                  <span className="font-mono text-xs font-bold text-amber-900 bg-amber-100 px-2 py-0.5 rounded">
                    {selectedSchema.prefix} ({selectedSchema.code})
                  </span>
                  <span className="text-[11px] text-blue-700 font-semibold">
                    {selectedSchema.layer}
                  </span>
                  <span className="text-[10px] bg-emerald-100 text-emerald-800 font-semibold px-2 py-0.5 rounded-full">
                    실시간 인스턴스: {currentInstances.length}건
                  </span>
                </div>
                <h3 className="text-base font-bold text-[var(--ink)] mt-1.5">{selectedSchema.name}</h3>
                <p className="text-xs text-[var(--steel)] mt-0.5">{selectedSchema.description}</p>
              </div>

              <Link href={selectedSchema.createUrl}>
                <Button size="sm" variant="brand" leftIcon={<PlusCircle size={14} />}>
                  {selectedSchema.createLabel}
                </Button>
              </Link>
            </div>

            {/* 인스턴스 검색 필터 */}
            <div className="flex items-center justify-between gap-3 pt-1">
              <div className="relative flex-1">
                <Search className="w-3.5 h-3.5 text-[var(--faint)] absolute left-2.5 top-2" />
                <input
                  type="text"
                  value={instanceSearch}
                  onChange={(e) => setInstanceSearch(e.target.value)}
                  placeholder="인스턴스 코드, 제목, 소속 검색..."
                  className="w-full pl-8 pr-2.5 py-1 text-xs rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] placeholder:text-[var(--faint)] focus:outline-none"
                />
              </div>
              <span className="text-[11px] text-[var(--steel)] shrink-0">
                표시: <strong>{filteredInstances.length}</strong> / {currentInstances.length}건
              </span>
            </div>

            {/* 실시간 인스턴스 인터랙티브 테이블 */}
            <div className="rounded-md border border-[var(--border)] overflow-hidden">
              <table className="w-full text-xs text-left border-collapse">
                <thead>
                  <tr className="border-b border-[var(--border)] bg-gray-50/70 text-[var(--steel)]">
                    <th className="px-3 py-2 font-medium">개체 코드</th>
                    <th className="px-3 py-2 font-medium">명칭 / 주요 내용</th>
                    <th className="px-3 py-2 font-medium">소속 / 맥락</th>
                    <th className="px-3 py-2 font-medium">핵심 지표</th>
                    <th className="px-3 py-2 font-medium">상태</th>
                    <th className="px-3 py-2 text-center font-medium">360° 인스펙터</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[var(--border-soft)]">
                  {filteredInstances.length === 0 ? (
                    <tr>
                      <td colSpan={6} className="p-6 text-center text-[var(--faint)]">
                        등록된 인스턴스가 없습니다.
                      </td>
                    </tr>
                  ) : (
                    filteredInstances.map((inst) => (
                      <tr key={inst.code} className="hover:bg-gray-50/80">
                        <td className="px-3 py-2">
                          <ObjectLink code={inst.code} />
                        </td>
                        <td className="px-3 py-2 font-semibold text-[var(--ink)] max-w-[180px] truncate">
                          {inst.title}
                        </td>
                        <td className="px-3 py-2 text-[var(--steel)] text-[11px]">
                          {inst.subtext}
                        </td>
                        <td className="px-3 py-2 font-mono text-[11px] text-[var(--ink)]">
                          {inst.amountOrMetric || "-"}
                        </td>
                        <td className="px-3 py-2">
                          <StatusChip label={inst.status} tone={inst.statusTone} size="xs" dot />
                        </td>
                        <td className="px-3 py-2 text-center">
                          <Button
                            size="xs"
                            variant="secondary"
                            leftIcon={<Eye size={12} />}
                            onClick={() => openObjectInspector(inst.code)}
                          >
                            열람
                          </Button>
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>

            {/* 표준 5갈래 인과 체인 상·하류 흐름 시각화 */}
            <div className="p-3.5 rounded-lg border border-[var(--border)] bg-gray-50/60 space-y-2">
              <div className="font-bold text-[11px] text-[var(--ink)] flex items-center justify-between">
                <div className="flex items-center gap-1.5">
                  <GitBranch className="w-3.5 h-3.5 text-amber-700" />
                  <span>단일 골격 인과 체인 상·하류 (Single Skeletal Causal Graph)</span>
                </div>
                <span className="text-[10px] text-[var(--faint)] font-mono">
                  All objects bind to persistent identity
                </span>
              </div>
              <div className="flex items-center gap-2 overflow-x-auto py-2 text-[11px]">
                <div className="p-2 rounded border bg-white shadow-2xs text-center shrink-0">
                  <span className="block font-mono font-bold text-emerald-800">C- 계약</span>
                  <span className="text-[10px] text-gray-500">수주·예산</span>
                </div>
                <ArrowRight className="w-3 h-3 text-gray-400 shrink-0" />
                <div className="p-2 rounded border bg-white shadow-2xs text-center shrink-0">
                  <span className="block font-mono font-bold text-slate-800">EMP- 인원</span>
                  <span className="text-[10px] text-gray-500">현장 배치</span>
                </div>
                <ArrowRight className="w-3 h-3 text-gray-400 shrink-0" />
                <div className="p-2 rounded border bg-white shadow-2xs text-center shrink-0">
                  <span className="block font-mono font-bold text-orange-800">AT- 근태</span>
                  <span className="text-[10px] text-gray-500">52h 산정</span>
                </div>
                <ArrowRight className="w-3 h-3 text-gray-400 shrink-0" />
                <div className="p-2 rounded border bg-white shadow-2xs text-center shrink-0">
                  <span className="block font-mono font-bold text-blue-800">AP- 결재</span>
                  <span className="text-[10px] text-gray-500">연장 승인</span>
                </div>
                <ArrowRight className="w-3 h-3 text-gray-400 shrink-0" />
                <div className="p-2 rounded border bg-white shadow-2xs text-center shrink-0">
                  <span className="block font-mono font-bold text-purple-800">PS- 급여</span>
                  <span className="text-[10px] text-gray-500">원가 환류</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
