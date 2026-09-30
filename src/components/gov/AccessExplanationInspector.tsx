"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { evaluateAccess } from "@/lib/policy-engine";
import { AccessEvaluationRequest, AccessExplanationResult } from "@/lib/types";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import { ReviewerHandoffModal } from "./ReviewerHandoffModal";
import {
  ShieldCheck,
  ShieldAlert,
  Search,
  CheckCircle2,
  AlertOctagon,
  KeyRound,
  User,
  Building,
  FileText,
  Clock,
  ArrowRight,
  HelpCircle,
  RotateCcw,
  Sparkles,
} from "lucide-react";

export function AccessExplanationInspector() {
  const employees = useAppStore((s) => s.employees);
  const approvals = useAppStore((s) => s.approvals);
  const payrollRun = useAppStore((s) => s.payrollRun);
  const cedarPolicies = useAppStore((s) => s.cedarPolicies);
  const accessAssignments = useAppStore((s) => s.accessAssignments);
  const capabilityBundles = useAppStore((s) => s.capabilityBundles);
  const addToast = useAppStore((s) => s.addToast);

  // Inspector form state
  const [selectedEmployeeId, setSelectedEmployeeId] = useState(employees[0]?.id || "emp_01");
  const [selectedAction, setSelectedAction] = useState("Payroll::ApproveRun");
  const [selectedResourceCode, setSelectedResourceCode] = useState("PR-2026-07");
  const [mfaVerified, setMfaVerified] = useState(true);
  const [handoffModalOpen, setHandoffModalOpen] = useState(false);

  const selectedEmployee = employees.find((e) => e.id === selectedEmployeeId);

  // Build simulated resource
  const getResourceDetails = () => {
    if (selectedResourceCode.startsWith("PR-")) {
      return {
        id: payrollRun.code,
        code: payrollRun.code,
        kind: "PR",
        entityId: "ENT-01",
        entityName: "(주)오야티",
        preparedByPersonId: "P-002", // 김인사
        preparedByName: "김인사",
        amount: payrollRun.totalGross,
        dataClassification: "민감" as const,
      };
    } else if (selectedResourceCode.startsWith("AP-")) {
      const doc = approvals.find((a) => a.code === selectedResourceCode) || approvals[0];
      return {
        id: doc.code,
        code: doc.code,
        kind: "AP",
        entityId: "ENT-01",
        entityName: "(주)오야티",
        preparedByPersonId: doc.drafterId,
        preparedByName: doc.drafterName,
        amount: doc.doaAmount || 15000000,
        dataClassification: "대외비" as const,
      };
    } else {
      return {
        id: "FB-202607-01",
        code: "FB-202607-01",
        kind: "FB",
        entityId: "ENT-01",
        entityName: "(주)오야티",
        preparedByPersonId: "P-003",
        preparedByName: "박재무",
        amount: 285400000,
        dataClassification: "비밀" as const,
      };
    }
  };

  const resource = getResourceDetails();

  // Evaluate request using Cedar evaluation kernel
  const req: AccessEvaluationRequest = {
    principalAccountId: selectedEmployee ? `ACC-${selectedEmployee.id}` : "ACC-GUEST",
    principalPersonId: selectedEmployee ? `P-${selectedEmployee.id}` : "P-GUEST",
    principalRoleGroup: selectedEmployee?.role || "사원",
    principalEntityId: selectedEmployee?.entity.includes("아크메") ? "ENT-02" : "ENT-01",
    action: selectedAction,
    resource,
    context: {
      time: new Date().toISOString(),
      mfaVerified,
    },
  };

  const result: AccessExplanationResult = evaluateAccess(
    req,
    cedarPolicies,
    accessAssignments,
    capabilityBundles
  );

  // Quick preset scenario loaders
  const loadScenario = (type: "sod_violation" | "normal_permit" | "tenant_violation" | "mfa_missing") => {
    if (type === "normal_permit") {
      // 박재무 (재무운영본부장) approving PR-2026-07 prepared by 김인사
      const emp = employees.find((e) => e.name.includes("박재무") || e.role.includes("본부장")) || employees[0];
      setSelectedEmployeeId(emp.id);
      setSelectedAction("Payroll::ApproveRun");
      setSelectedResourceCode("PR-2026-07");
      setMfaVerified(true);
      addToast({ title: "시나리오 적용", description: "정상 인가 시나리오가 로드되었습니다.", tone: "ok" });
    } else if (type === "sod_violation") {
      // 김인사 (인사기획팀장) attempts to approve PR-2026-07 which she prepared!
      const emp = employees.find((e) => e.name.includes("김인사") || e.role.includes("팀장")) || employees[1];
      setSelectedEmployeeId(emp.id);
      setSelectedAction("Payroll::ApproveRun");
      setSelectedResourceCode("PR-2026-07");
      setMfaVerified(true);
      addToast({ title: "시나리오 적용", description: "직무 분리(SoD) 위반 차단 시나리오가 로드되었습니다.", tone: "warn" });
    } else if (type === "tenant_violation") {
      // Acme Logis employee accessing Oyatie doc
      const emp = employees.find((e) => e.entity.includes("아크메") || e.site.includes("부산")) || employees[2];
      setSelectedEmployeeId(emp.id);
      setSelectedAction("Approvals::SignTier1");
      setSelectedResourceCode("AP-3121");
      setMfaVerified(true);
      addToast({ title: "시나리오 적용", description: "타 법인 경계 위반 차단 시나리오가 로드되었습니다.", tone: "warn" });
    } else if (type === "mfa_missing") {
      const emp = employees[0];
      setSelectedEmployeeId(emp.id);
      setSelectedAction("Approvals::SignTier3");
      setSelectedResourceCode("AP-3121");
      setMfaVerified(false);
      addToast({ title: "시나리오 적용", description: "Passkey 미인증 차단 시나리오가 로드되었습니다.", tone: "warn" });
    }
  };

  return (
    <div className="space-y-6">
      {/* 1. 빠른 시나리오 프리셋 버튼 바 */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 shadow-2xs space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Sparkles className="w-4 h-4 text-indigo-600" />
            <h4 className="font-bold text-xs text-[var(--ink)]">
              대표 내부통제 검증 시나리오 (1-Click Evaluation Scenarios)
            </h4>
          </div>
          <span className="text-[11px] text-[var(--steel)]">
            Cedar 정책 엔진의 결정 근거와 자가 치유를 즉시 시뮬레이션합니다.
          </span>
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-2">
          <button
            onClick={() => loadScenario("normal_permit")}
            className="px-3 py-2 rounded-md border border-[var(--border)] bg-[var(--canvas)] hover:border-emerald-500 hover:bg-emerald-50/30 text-left text-xs transition cursor-pointer"
          >
            <div className="font-bold text-emerald-800 flex items-center gap-1.5">
              <CheckCircle2 className="w-3.5 h-3.5 text-emerald-600" />
              <span>정상 인가 (Permit)</span>
            </div>
            <p className="text-[10px] text-[var(--steel)] mt-0.5">본부장의 타인 기안 급여대장 승인</p>
          </button>

          <button
            onClick={() => loadScenario("sod_violation")}
            className="px-3 py-2 rounded-md border border-[var(--border)] bg-[var(--canvas)] hover:border-red-500 hover:bg-red-50/30 text-left text-xs transition cursor-pointer"
          >
            <div className="font-bold text-red-800 flex items-center gap-1.5">
              <AlertOctagon className="w-3.5 h-3.5 text-red-600" />
              <span>SoD 위반 차단</span>
            </div>
            <p className="text-[10px] text-[var(--steel)] mt-0.5">기안자의 자가 기안 문서 자가 승인 금지</p>
          </button>

          <button
            onClick={() => loadScenario("mfa_missing")}
            className="px-3 py-2 rounded-md border border-[var(--border)] bg-[var(--canvas)] hover:border-amber-500 hover:bg-amber-50/30 text-left text-xs transition cursor-pointer"
          >
            <div className="font-bold text-amber-800 flex items-center gap-1.5">
              <KeyRound className="w-3.5 h-3.5 text-amber-600" />
              <span>Passkey 미인증 차단</span>
            </div>
            <p className="text-[10px] text-[var(--steel)] mt-0.5">고액 결재/자금집행 생체인증 누락</p>
          </button>

          <button
            onClick={() => loadScenario("tenant_violation")}
            className="px-3 py-2 rounded-md border border-[var(--border)] bg-[var(--canvas)] hover:border-purple-500 hover:bg-purple-50/30 text-left text-xs transition cursor-pointer"
          >
            <div className="font-bold text-purple-800 flex items-center gap-1.5">
              <Building className="w-3.5 h-3.5 text-purple-600" />
              <span>법인 경계 월경 차단</span>
            </div>
            <p className="text-[10px] text-[var(--steel)] mt-0.5">타 계열사 자원에 대한 무단 접근</p>
          </button>
        </div>
      </div>

      {/* 2. 인터랙티브 인가 진단기 입력 패널 */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-5">
        {/* 입력 제어 카드 */}
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-2.5">
            <h4 className="font-bold text-xs text-[var(--ink)] flex items-center gap-2">
              <Search className="w-4 h-4 text-indigo-600" />
              <span>권한 진단 대상 설정 (Evaluation Context)</span>
            </h4>
          </div>

          <div className="space-y-3 text-xs">
            {/* 1. 작업 수행자 (Principal) */}
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)] flex items-center justify-between">
                <span>수행 대상 임직원 (Principal)</span>
                <span className="text-[10px] font-mono text-[var(--steel)]">PersonIdentity</span>
              </label>
              <select
                value={selectedEmployeeId}
                onChange={(e) => setSelectedEmployeeId(e.target.value)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-xs"
              >
                {employees.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.name} ({e.role} · {e.dept} · {e.entity})
                  </option>
                ))}
              </select>
              {selectedEmployee && (
                <div className="mt-1 text-[11px] text-[var(--steel)] flex items-center gap-2">
                  <span>자연인 ID: <strong className="font-mono text-[var(--ink)]">P-{selectedEmployee.id}</strong></span>
                  <span>· 계정: <strong className="font-mono text-[var(--ink)]">ACC-{selectedEmployee.id}</strong></span>
                </div>
              )}
            </div>

            {/* 2. 수행할 업무 행위 (Action) */}
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">수행하려는 업무 행위 (Action)</label>
              <select
                value={selectedAction}
                onChange={(e) => setSelectedAction(e.target.value)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-xs font-mono"
              >
                <option value="Payroll::ApproveRun">Payroll::ApproveRun (정기 급여 대장 승인)</option>
                <option value="Payroll::ExecuteFirmBanking">Payroll::ExecuteFirmBanking (펌뱅킹 모계좌 출금 이체)</option>
                <option value="Approvals::SignTier1">Approvals::SignTier1 (1단계 일상경비 전결)</option>
                <option value="Approvals::SignTier2">Approvals::SignTier2 (2단계 부품계약 전결)</option>
                <option value="Approvals::SignTier3">Approvals::SignTier3 (3단계 고액 지출 최종승인)</option>
                <option value="HR::TransferEmployee">HR::TransferEmployee (부서 전보 발령 기안/승인)</option>
                <option value="Operations::AssignWorkOrder">Operations::AssignWorkOrder (작업오더 배차)</option>
              </select>
            </div>

            {/* 3. 대상 업무 개체 (Resource) */}
            <div>
              <label className="font-bold block mb-1 text-[var(--ink)]">대상 업무 자원 (Resource)</label>
              <select
                value={selectedResourceCode}
                onChange={(e) => setSelectedResourceCode(e.target.value)}
                className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] text-xs font-mono"
              >
                <option value="PR-2026-07">PR-2026-07 (7월 정기 급여대장 · 기안자: 김인사)</option>
                <option value="AP-3121">AP-3121 (정비 모터 교체 품의 · 기안자: 이수력)</option>
                <option value="FB-202607-01">FB-202607-01 (7월 급여 펌뱅킹 이체 배치)</option>
              </select>
              <div className="mt-1 text-[11px] text-[var(--steel)]">
                귀속 법인: <strong className="text-[var(--ink)]">{resource.entityName}</strong> · 기안자: <strong className="text-[var(--ink)]">{resource.preparedByName}</strong>
              </div>
            </div>

            {/* 4. 인증 컨텍스트 (Context) */}
            <div className="pt-2 border-t border-[var(--border)]/60">
              <label className="font-bold block mb-1 text-[var(--ink)]">세션 보안 컨텍스트</label>
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={mfaVerified}
                  onChange={(e) => setMfaVerified(e.target.checked)}
                  className="rounded text-[var(--brand)] focus:ring-0"
                />
                <span className="text-xs text-[var(--ink)]">
                  FIDO2 Passkey 생체 인증 완료 세션 (Hardware Authenticated)
                </span>
              </label>
            </div>
          </div>
        </div>

        {/* 진단 결과 카드 (2/3 width) */}
        <div className="lg:col-span-2 rounded-lg border border-[var(--border)] bg-[var(--surface)] p-5 shadow-2xs space-y-4">
          <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
            <div className="flex items-center gap-2.5">
              {result.decision === "PERMIT" ? (
                <div className="w-8 h-8 rounded-full bg-emerald-500/10 text-emerald-600 flex items-center justify-center">
                  <CheckCircle2 className="w-5 h-5" />
                </div>
              ) : (
                <div className="w-8 h-8 rounded-full bg-red-500/10 text-red-600 flex items-center justify-center">
                  <ShieldAlert className="w-5 h-5" />
                </div>
              )}
              <div>
                <div className="flex items-center gap-2">
                  <h3 className="font-bold text-base text-[var(--ink)]">
                    {result.decision === "PERMIT" ? "인가 승인 (PERMIT)" : "인가 거부 (FORBID)"}
                  </h3>
                  <StatusChip
                    label={result.decision === "PERMIT" ? "통과" : "차단"}
                    tone={result.decision === "PERMIT" ? "ok" : "danger"}
                    size="sm"
                  />
                </div>
                <p className="text-xs text-[var(--steel)] mt-0.5">{result.summary}</p>
              </div>
            </div>
            <span className="text-[10px] font-mono text-[var(--faint)]">
              {result.evaluatedAt.slice(11, 19)} 진단됨
            </span>
          </div>

          {/* 인가 트리 및 규칙 매칭 상세 */}
          <div className="space-y-3 text-xs">
            {/* A. 차단 사유 (Blocking Forbid Policies) */}
            {result.blockingForbidRules.length > 0 && (
              <div className="rounded-lg border border-red-200 bg-red-50/50 p-3.5 space-y-2">
                <div className="font-bold text-red-900 flex items-center gap-1.5 text-xs">
                  <AlertOctagon className="w-4 h-4 text-red-600" />
                  <span>내부통제 강행 차단 규정 ({result.blockingForbidRules.length}건 위반)</span>
                </div>
                <div className="space-y-1.5">
                  {result.blockingForbidRules.map((r) => (
                    <div key={r.id} className="p-2.5 rounded bg-white border border-red-200 text-[11px] space-y-1">
                      <div className="font-bold text-red-950 flex items-center justify-between">
                        <span>{r.title}</span>
                        <span className="font-mono text-[10px] text-red-700 font-bold">[FORBID OVERRIDE]</span>
                      </div>
                      <p className="text-red-900 leading-relaxed">{r.violationReason}</p>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* B. 부합된 허용 규칙 (Matched Permit Grants) */}
            <div className="rounded-lg border border-[var(--border)] bg-[var(--canvas)] p-3.5 space-y-2">
              <div className="font-bold text-[var(--ink)] flex items-center justify-between">
                <span className="flex items-center gap-1.5">
                  <ShieldCheck className="w-4 h-4 text-emerald-600" />
                  <span>부합된 권한 번들 (Permit Grants: {result.matchedPermitRules.length}건)</span>
                </span>
                <span className="text-[10px] text-[var(--steel)]">Default-Deny Semantics</span>
              </div>
              {result.matchedPermitRules.length > 0 ? (
                <div className="space-y-1.5">
                  {result.matchedPermitRules.map((p) => (
                    <div key={p.id} className="p-2.5 rounded bg-[var(--surface)] border border-[var(--border)] text-[11px]">
                      <div className="font-bold text-[var(--ink)]">{p.title}</div>
                      <div className="text-[10px] text-[var(--steel)] mt-0.5">{p.scope}</div>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="text-[11px] text-[var(--steel)] italic">
                  해당 행위를 인가하는 활성 직무 권한(Capability Assignment)이 존재하지 않습니다.
                </p>
              )}
            </div>

            {/* C. 자가 치유 조치 가이드 (Remediation Advice) */}
            {result.remediationAdvice && (
              <div className="rounded-lg border border-indigo-200 bg-indigo-50/50 p-3.5 space-y-2.5">
                <div className="font-bold text-indigo-950 flex items-center justify-between">
                  <span className="flex items-center gap-1.5">
                    <HelpCircle className="w-4 h-4 text-indigo-700" />
                    <span>권장 자가 치유 및 조치 가이드 (Safe Remediation)</span>
                  </span>
                  <StatusChip
                    label={result.remediationAdvice.canSelfRemediate ? "즉시 치유 가능" : "인계/신청 필요"}
                    tone={result.remediationAdvice.canSelfRemediate ? "ok" : "info"}
                    size="xs"
                  />
                </div>
                <p className="text-[11px] text-indigo-950 leading-relaxed">
                  {result.remediationAdvice.recommendedAction}
                </p>

                {/* 상황별 즉각 액션 버튼 */}
                <div className="pt-1 flex items-center gap-2">
                  {result.remediationAdvice.actionType === "handoff_delegate" && (
                    <Button
                      size="xs"
                      variant="brand"
                      leftIcon={<User className="w-3.5 h-3.5" />}
                      onClick={() => setHandoffModalOpen(true)}
                    >
                      부재자 대결(Delegate) 인계 모달 열기
                    </Button>
                  )}

                  {result.remediationAdvice.actionType === "passkey_register" && (
                    <Button
                      size="xs"
                      variant="brand"
                      leftIcon={<KeyRound className="w-3.5 h-3.5" />}
                      onClick={() => {
                        setMfaVerified(true);
                        addToast({
                          title: "Passkey 인증 완료",
                          description: "FIDO2 Touch ID 생체인증이 검증되어 고위험 전결 게이트가 개방되었습니다.",
                          tone: "ok",
                        });
                      }}
                    >
                      FIDO2 Passkey 생체인증 즉시 승격
                    </Button>
                  )}

                  {result.remediationAdvice.actionType === "switch_entity" && (
                    <Button
                      size="xs"
                      variant="brand"
                      leftIcon={<Building className="w-3.5 h-3.5" />}
                      onClick={() => {
                        addToast({
                          title: "법인 전환 안내",
                          description: "상단 법인 스위처에서 (주)오야티로 전환하십시오.",
                          tone: "info",
                        });
                      }}
                    >
                      해당 법인으로 작업 전환
                    </Button>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* 부재자 대결 모달 연결 */}
      <ReviewerHandoffModal
        isOpen={handoffModalOpen}
        onClose={() => setHandoffModalOpen(false)}
        preselectedDocCode={selectedResourceCode.startsWith("AP-") ? selectedResourceCode : undefined}
      />
    </div>
  );
}
