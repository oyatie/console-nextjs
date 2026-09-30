"use client";

import React from "react";
import Link from "next/link";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { StatusChip } from "@/components/ui/StatusChip";
import {
  FileText,
  Clock,
  DollarSign,
  Users,
  Wrench,
  AlertTriangle,
  CheckCircle2,
  ArrowRight,
  TrendingUp,
  ShieldCheck,
  Zap,
  Calendar,
  Layers,
  Building,
  RotateCcw,
  Sparkles,
  ArrowUpRight,
} from "lucide-react";

export default function DashboardPage() {
  const employees = useAppStore((s) => s.employees);
  const attendance = useAppStore((s) => s.attendance);
  const approvals = useAppStore((s) => s.approvals);
  const workOrders = useAppStore((s) => s.workOrders);
  const payrollRun = useAppStore((s) => s.payrollRun);
  const bankBatch = useAppStore((s) => s.bankTransferBatch);
  const orgEntities = useAppStore((s) => s.orgEntities);
  const activeEntityId = useAppStore((s) => s.activeEntityId);
  const setCommandPaletteOpen = useAppStore((s) => s.setCommandPaletteOpen);
  const approveDoc = useAppStore((s) => s.approveDoc);

  // 현황 데이터 필터링
  const pendingApprovals = approvals.filter((a) => a.status === "결재대기");
  const attendanceExceptions = attendance.filter((a) => a.exceptionCode);
  const urgentOrders = workOrders.filter((w) => w.priority === "긴급" && w.status !== "완료");
  const activeTechnicians = employees.filter((e) => e.status === "재직").length;

  const currentEntity = orgEntities.find((e) => e.id === activeEntityId) || orgEntities[0];
  const companyName = activeEntityId === "all" ? "Acme Group 전체" : currentEntity?.name || "(주)오야티 코퍼레이션";

  return (
    <div className="space-y-6">
      {/* 1. 상단 사령탑 헤더: 현재 작업 공간 및 단축키 */}
      <div className="flex flex-wrap items-center justify-between gap-3 p-4 rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs">
        <div className="flex items-center gap-3">
          <div className="w-9 h-9 rounded-lg bg-amber-500/15 flex items-center justify-center text-[var(--signal-deep)] font-bold shadow-xs">
            <Zap className="w-5 h-5" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h2 className="text-sm font-bold text-[var(--ink)]">
                {companyName} · 업무 사령탑
              </h2>
              <span className="px-2 py-0.5 rounded text-[10px] font-mono font-bold bg-amber-100 text-amber-900 border border-amber-200">
                실시간 엔티티 엔진
              </span>
            </div>
            <p className="text-xs text-[var(--steel)] mt-0.5">
              지금 주의가 필요한 업무와 진행 중인 작업을 한눈에 확인하고 즉시 이어갈 수 있습니다.
            </p>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="brand"
            onClick={() => setCommandPaletteOpen(true)}
            leftIcon={<Zap className="w-3.5 h-3.5" />}
          >
            빠른 커맨드 (⌘K)
          </Button>
          <Link href="/approvals">
            <Button size="sm" variant="secondary">
              결재 대장 보기
            </Button>
          </Link>
        </div>
      </div>

      {/* 2. 핵심 지표 1행 스탯 바 */}
      <StatBar
        items={[
          {
            label: "재직 인원",
            value: `${activeTechnicians}명`,
            badge: "정상 가동",
            badgeTone: "ok",
          },
          {
            label: "결재 대기",
            value: `${pendingApprovals.length}건`,
            badge: pendingApprovals.length > 0 ? "심사 요망" : "완료",
            badgeTone: pendingApprovals.length > 0 ? "warn" : "ok",
          },
          {
            label: "주52h 근태 예외",
            value: `${attendanceExceptions.length}건`,
            badge: attendanceExceptions.length > 0 ? "급여 블로커" : "게이트 통과",
            badgeTone: attendanceExceptions.length > 0 ? "danger" : "ok",
          },
          {
            label: "긴급 작업오더",
            value: `${urgentOrders.length}건`,
            badge: urgentOrders.length > 0 ? "SLA 주의" : "안정",
            badgeTone: urgentOrders.length > 0 ? "danger" : "info",
          },
          {
            label: "7월 급여 펌뱅킹",
            value: bankBatch.status,
            badge: bankBatch.status === "출금승인대기" ? "봉인 완료" : "준비중",
            badgeTone: bankBatch.status === "출금승인대기" ? "ok" : "warn",
          },
        ]}
      />

      {/* 3. 4대 핵심 질문 영역 (Tenet 1: What needs attention, What am I working on, What happens next, Where did I leave off) */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-5">
        {/* 질문 1: 지금 나의 주의가 필요한 일은 무엇인가? (What needs my attention?) */}
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 space-y-3 shadow-2xs">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-2.5">
            <div className="flex items-center gap-2">
              <AlertTriangle className="w-4 h-4 text-amber-600" />
              <h3 className="text-xs font-bold text-[var(--ink)]">
                지금 주의가 필요한 업무 (What Needs Attention)
              </h3>
              <span className="px-1.5 py-0.2 rounded bg-red-100 text-red-800 text-[10px] font-mono font-bold">
                {pendingApprovals.length + (payrollRun.unresolvedExceptions > 0 ? 1 : 0) + urgentOrders.length}건
              </span>
            </div>
            <span className="text-[11px] text-[var(--faint)]">마감 블로커 우선</span>
          </div>

          <div className="space-y-2.5">
            {/* 급여 마감 블로커 카드 */}
            {payrollRun.unresolvedExceptions > 0 && (
              <div className="p-3 rounded-md border border-amber-300 bg-amber-50/70 flex items-start justify-between gap-3">
                <div className="space-y-1 text-xs">
                  <div className="flex items-center gap-2">
                    <span className="font-bold text-amber-950">
                      2026-07 월 급여 산정 및 펌뱅킹 준비 · {companyName}
                    </span>
                    <span className="px-1.5 py-0.2 rounded bg-amber-200 text-amber-900 text-[10px] font-semibold">
                      급여 락 차단
                    </span>
                  </div>
                  <p className="text-[11px] text-amber-900 leading-relaxed">
                    근로기준법 제53조 위반 방지를 위해 근태 예외 {payrollRun.unresolvedExceptions}건(주52시간 초과 소명)이 먼저 심사되어야 합니다.
                  </p>
                </div>
                <Link href="/hr/attendance" className="shrink-0">
                  <Button size="xs" variant="brand">
                    예외 심사 →
                  </Button>
                </Link>
              </div>
            )}

            {/* 결재 대기 건들 */}
            {pendingApprovals.slice(0, 3).map((doc) => (
              <div
                key={doc.id}
                className="p-3 rounded-md border border-[var(--border)] hover:border-blue-300 transition-colors flex items-center justify-between gap-3 bg-[var(--canvas)]"
              >
                <div className="space-y-0.5 min-w-0">
                  <div className="flex items-center gap-2">
                    <ObjectLink code={doc.code} />
                    <span className="text-xs font-semibold text-[var(--ink)] truncate">
                      {doc.title}
                    </span>
                    <StatusChip label={doc.category} tone="info" size="xs" />
                  </div>
                  <div className="text-[11px] text-[var(--steel)]">
                    기안자: {doc.drafterName} ({doc.drafterDept}) · 결재 기한: 금일 18:00
                  </div>
                </div>

                <div className="flex items-center gap-1.5 shrink-0">
                  <Button
                    size="xs"
                    variant="brand"
                    onClick={() => approveDoc(doc.id, "인사노무관리자 (나)", "사령탑 즉시 승인")}
                  >
                    1-클릭 승인
                  </Button>
                  <Link href="/approvals">
                    <Button size="xs" variant="outline">
                      검토
                    </Button>
                  </Link>
                </div>
              </div>
            ))}

            {/* 긴급 작업 오더 건 */}
            {urgentOrders.slice(0, 1).map((wo) => (
              <div
                key={wo.id}
                className="p-3 rounded-md border border-red-200 bg-red-50/40 flex items-center justify-between gap-3 text-xs"
              >
                <div>
                  <div className="flex items-center gap-2">
                    <ObjectLink code={wo.code} />
                    <span className="font-bold text-red-950 truncate">{wo.title}</span>
                    <StatusChip label="긴급 SLA" tone="danger" size="xs" />
                  </div>
                  <div className="text-[11px] text-red-900 mt-0.5">
                    거점: {wo.site} · 목표 복구 시간: 45분 이내 현장 배차 필요
                  </div>
                </div>
                <Link href="/ops/work-orders">
                  <Button size="xs" variant="secondary">
                    배차 배정 →
                  </Button>
                </Link>
              </div>
            ))}
          </div>
        </div>

        {/* 질문 2 & 4: 현재 무엇을 작업 중이고 어디서 멈추었는가? (What am I working on & Where did I leave off?) */}
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 space-y-3 shadow-2xs">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-2.5">
            <div className="flex items-center gap-2">
              <RotateCcw className="w-4 h-4 text-blue-600" />
              <h3 className="text-xs font-bold text-[var(--ink)]">
                진행 중인 작업 및 저장된 초안 (Where I Left Off)
              </h3>
            </div>
            <span className="text-[11px] text-[var(--faint)]">저장 상태 보존됨</span>
          </div>

          <div className="space-y-2.5">
            {/* 이어서 작업하기: 7월 정기 급여 펌뱅킹 덱 */}
            <div className="p-3 rounded-md border border-blue-200 bg-blue-50/50 space-y-2 text-xs">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <DollarSign className="w-4 h-4 text-blue-700" />
                  <span className="font-bold text-blue-950">
                    2026-07 월 급여 준비 원장 ({payrollRun.code})
                  </span>
                </div>
                <span className="font-mono text-[11px] text-blue-800 bg-blue-100 px-1.5 py-0.2 rounded font-semibold">
                  {payrollRun.status}
                </span>
              </div>
              <p className="text-[11px] text-blue-900">
                총 {employees.length}명 산정 완료 · 금융결제원 100-byte 펌뱅킹 전문 생성 대기 상태로 저장되어 있습니다.
              </p>
              <div className="flex items-center justify-between pt-1">
                <span className="text-[10px] text-blue-700 font-mono">
                  출금 모계좌: {bankBatch.masterBank} {bankBatch.masterAccount}
                </span>
                <Link href="/hr/payroll">
                  <Button size="xs" variant="brand" leftIcon={<ArrowRight className="w-3 h-3" />}>
                    급여 작업 이어하기
                  </Button>
                </Link>
              </div>
            </div>

            {/* 이어서 작업하기: 최근 품의 기안 초안 */}
            <div className="p-3 rounded-md border border-[var(--border)] bg-[var(--canvas)] space-y-1.5 text-xs">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <FileText className="w-4 h-4 text-[var(--steel)]" />
                  <span className="font-semibold text-[var(--ink)]">
                    AP-3122 인천센터 물류 자동화 설비 3분기 예산 품의
                  </span>
                </div>
                <span className="text-[10px] font-mono text-gray-500 bg-gray-100 px-1.5 py-0.2 rounded">
                  임시저장
                </span>
              </div>
              <p className="text-[11px] text-[var(--steel)]">
                품목 명세 3건 작성 완료 · 전결 2등급(3,500만원) 결재선 설정 중 보존되었습니다.
              </p>
              <div className="flex justify-end pt-1">
                <Link href="/approvals">
                  <Button size="xs" variant="secondary">
                    기안 작성 이어하기 →
                  </Button>
                </Link>
              </div>
            </div>

            {/* 이어서 작업하기: 인사명령 발령 초안 */}
            <div className="p-3 rounded-md border border-[var(--border)] bg-[var(--canvas)] space-y-1.5 text-xs">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <Users className="w-4 h-4 text-[var(--steel)]" />
                  <span className="font-semibold text-[var(--ink)]">
                    PA-2026-44 하반기 신규 엔지니어 발령 대기
                  </span>
                </div>
                <span className="text-[10px] font-mono text-gray-500 bg-gray-100 px-1.5 py-0.2 rounded">
                  결재 진행중
                </span>
              </div>
              <p className="text-[11px] text-[var(--steel)]">
                설비운영팀 배치 및 통상임금 호봉 산정서 결재 완료 시 사원 원장에 즉시 전파됩니다.
              </p>
              <div className="flex justify-end pt-1">
                <Link href="/hr/actions">
                  <Button size="xs" variant="secondary">
                    발령 대장 확인 →
                  </Button>
                </Link>
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* 질문 3: 다음 단계 및 주요 업무 일정은 무엇인가? (What happens next?) */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 space-y-3 shadow-2xs">
        <div className="flex items-center justify-between border-b border-[var(--border)] pb-2.5">
          <div className="flex items-center gap-2">
            <Calendar className="w-4 h-4 text-emerald-600" />
            <h3 className="text-xs font-bold text-[var(--ink)]">
              다음 단계 및 다가오는 법정 일정 (What Happens Next)
            </h3>
          </div>
          <span className="text-[11px] text-[var(--steel)]">2026년 7월 ~ 8월 주요 마일스톤</span>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs">
          <div className="p-3 rounded border border-emerald-200 bg-emerald-50/50 space-y-1">
            <div className="flex items-center justify-between">
              <span className="font-bold text-emerald-950">7월 25일 (금)</span>
              <span className="text-[10px] font-mono text-emerald-800 bg-emerald-100 px-1.5 py-0.2 rounded">
                D-3일
              </span>
            </div>
            <div className="font-semibold text-emerald-900">2026-07 월 정기 급여 펌뱅킹 이체일</div>
            <p className="text-[11px] text-emerald-800">
              전 사원 실명인증 계좌로 펌뱅킹 대량 출금 전송 및 근로기준법 제48조 표준 임금명세서 일괄 교부.
            </p>
          </div>

          <div className="p-3 rounded border border-blue-200 bg-blue-50/50 space-y-1">
            <div className="flex items-center justify-between">
              <span className="font-bold text-blue-950">7월 31일 (목)</span>
              <span className="text-[10px] font-mono text-blue-800 bg-blue-100 px-1.5 py-0.2 rounded">
                D-9일
              </span>
            </div>
            <div className="font-semibold text-blue-900">연차유급휴가 1차 촉진 통지 마감</div>
            <p className="text-[11px] text-blue-800">
              근로기준법 제61조에 따라 잔여연차 보유자 대상 FIDO2 Passkey 수령확인 통지서 발송 및 수령 증명 보관.
            </p>
          </div>

          <div className="p-3 rounded border border-purple-200 bg-purple-50/50 space-y-1">
            <div className="flex items-center justify-between">
              <span className="font-bold text-purple-950">8월 01일 (금)</span>
              <span className="text-[10px] font-mono text-purple-800 bg-purple-100 px-1.5 py-0.2 rounded">
                D-10일
              </span>
            </div>
            <div className="font-semibold text-purple-900">하반기 조직개편 & 정기 승진 발령일</div>
            <p className="text-[11px] text-purple-800">
              인사명령 대장(PA-2026) 승진 및 부서 전보 내역이 사원 원장 및 권한 정책에 일괄 자동 적용.
            </p>
          </div>
        </div>
      </div>

      {/* 4. 신규 워크스페이스 시작 가이드 (신규 사용자 및 비어있는 환경을 위한 안내) */}
      <div className="p-4 rounded-lg border border-[var(--border)] bg-gradient-to-r from-gray-50 via-surface to-amber-50/30 space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Building className="w-4 h-4 text-amber-600" />
            <h3 className="text-xs font-bold text-[var(--ink)]">
              처음 시작하거나 조직 구조를 확장할 때 (Setup Principles)
            </h3>
          </div>
          <span className="text-[11px] text-[var(--steel)]">사전 입력된 관계 우선 · 누락된 결정만 질문</span>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3 text-xs">
          <Link
            href="/org/setup"
            className="p-3 rounded border border-[var(--border)] bg-[var(--surface)] hover:border-amber-400 hover:shadow-xs transition-all group"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="font-bold text-[var(--ink)]">1. 조직 체계 설정</span>
              <ArrowUpRight className="w-3.5 h-3.5 text-[var(--steel)] group-hover:text-amber-600 transition-colors" />
            </div>
            <p className="text-[11px] text-[var(--steel)]">
              법인(OrgEntity), 거점 사업장(Site), 부서 트리 및 직급 사다리를 정의합니다.
            </p>
          </Link>

          <Link
            href="/org/policy"
            className="p-3 rounded border border-[var(--border)] bg-[var(--surface)] hover:border-amber-400 hover:shadow-xs transition-all group"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="font-bold text-[var(--ink)]">2. 사규 · 전결 정책</span>
              <ArrowUpRight className="w-3.5 h-3.5 text-[var(--steel)] group-hover:text-amber-600 transition-colors" />
            </div>
            <p className="text-[11px] text-[var(--steel)]">
              주52시간 게이트, §54 휴게시간, 전결 규정(DoA Tiers), 연차 촉진 일정을 승인합니다.
            </p>
          </Link>

          <Link
            href="/hr/people"
            className="p-3 rounded border border-[var(--border)] bg-[var(--surface)] hover:border-amber-400 hover:shadow-xs transition-all group"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="font-bold text-[var(--ink)]">3. 직원 및 고용 등록</span>
              <ArrowUpRight className="w-3.5 h-3.5 text-[var(--steel)] group-hover:text-amber-600 transition-colors" />
            </div>
            <p className="text-[11px] text-[var(--steel)]">
              신규 사원을 등록하거나 Excel TSV 복사-붙여넣기로 대량 사원을 일괄 등록합니다.
            </p>
          </Link>

          <Link
            href="/hr/payroll"
            className="p-3 rounded border border-[var(--border)] bg-[var(--surface)] hover:border-amber-400 hover:shadow-xs transition-all group"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="font-bold text-[var(--ink)]">4. 급여 및 펌뱅킹 준비</span>
              <ArrowUpRight className="w-3.5 h-3.5 text-[var(--steel)] group-hover:text-amber-600 transition-colors" />
            </div>
            <p className="text-[11px] text-[var(--steel)]">
              근태 마감 검증 후 4대보험 자동 계산 및 금융결제원 100바이트 전문을 생성합니다.
            </p>
          </Link>
        </div>
      </div>
    </div>
  );
}
