"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { AccessExplanationInspector } from "@/components/gov/AccessExplanationInspector";
import { ConstrainedPolicyBuilder } from "@/components/gov/ConstrainedPolicyBuilder";
import { OperationalRecoveryCenter } from "@/components/gov/OperationalRecoveryCenter";
import { ReviewerHandoffModal } from "@/components/gov/ReviewerHandoffModal";
import { LayeredRoleCanvas } from "@/components/gov/LayeredRoleCanvas";
import {
  ShieldCheck,
  Search,
  Users,
  Wrench,
  UserCheck,
  Sliders,
  CheckCircle2,
  Layers,
} from "lucide-react";

export default function AccessGovernancePage() {
  const cedarPolicies = useAppStore((s) => s.cedarPolicies);
  const accessAssignments = useAppStore((s) => s.accessAssignments);
  const operationalIncidents = useAppStore((s) => s.operationalIncidents);
  const roleDefinitions = useAppStore((s) => s.roleDefinitions);

  const [activeTab, setActiveTab] = useState<"roles" | "inspector" | "builder" | "recovery">("roles");
  const [handoffOpen, setHandoffOpen] = useState(false);

  const activeGrantsCount = accessAssignments.filter((a) => a.status === "active").length;
  const openIncidentsCount = operationalIncidents.filter((i) => i.status === "open").length;

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          {
            label: "Cedar 보안 사규",
            value: `${cedarPolicies.length}개 강행 규칙`,
            badge: "SoD 분리 적용",
            badgeTone: "ok",
          },
          {
            label: "활성 직무 권한 배정",
            value: `${activeGrantsCount}건 활성`,
            subValue: "Default-Deny 기준",
          },
          {
            label: "운영 장애 모니터링",
            value: `${openIncidentsCount}건 대기`,
            badgeTone: openIncidentsCount > 0 ? "warn" : "ok",
          },
          {
            label: "내부회계 평가",
            value: "적정 (Compliant)",
            badge: "K-SOX 준수",
            badgeTone: "ok",
          },
        ]}
      />

      {/* 2. 탭 네비게이션 및 대결 액션 버튼 */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 border-b border-[var(--border)] pb-2">
        <div className="flex items-center gap-1.5 overflow-x-auto">
          <button
            onClick={() => setActiveTab("roles")}
            className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
              activeTab === "roles"
                ? "bg-indigo-600 text-white shadow-2xs"
                : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
            }`}
          >
            <Layers className="w-4 h-4" />
            <span>다중 역할 레이어 카탈로그 (Discord Model: {roleDefinitions.length})</span>
          </button>

          <button
            onClick={() => setActiveTab("inspector")}
            className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
              activeTab === "inspector"
                ? "bg-indigo-600 text-white shadow-2xs"
                : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
            }`}
          >
            <Search className="w-4 h-4" />
            <span>접근 권한 진단기 (Access Inspector)</span>
          </button>

          <button
            onClick={() => setActiveTab("builder")}
            className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
              activeTab === "builder"
                ? "bg-indigo-600 text-white shadow-2xs"
                : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
            }`}
          >
            <Users className="w-4 h-4" />
            <span>비기술 직무 권한 배정기 (Policy Builder)</span>
          </button>

          <button
            onClick={() => setActiveTab("recovery")}
            className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-bold transition cursor-pointer shrink-0 ${
              activeTab === "recovery"
                ? "bg-indigo-600 text-white shadow-2xs"
                : "text-[var(--steel)] hover:bg-[var(--muted)] hover:text-[var(--ink)]"
            }`}
          >
            <Wrench className="w-4 h-4" />
            <span>운영 장애 셀프 복구 센터 ({openIncidentsCount})</span>
          </button>
        </div>

        <Button
          size="sm"
          variant="outline"
          leftIcon={<UserCheck className="w-4 h-4 text-amber-600" />}
          onClick={() => setHandoffOpen(true)}
        >
          부재자 결재 대결 지정
        </Button>
      </div>

      {/* 3. 탭별 컨텐츠 렌더링 */}
      {activeTab === "roles" && <LayeredRoleCanvas />}
      {activeTab === "inspector" && <AccessExplanationInspector />}
      {activeTab === "builder" && <ConstrainedPolicyBuilder />}
      {activeTab === "recovery" && <OperationalRecoveryCenter />}

      {/* 부재자 결재 대결 모달 */}
      <ReviewerHandoffModal
        isOpen={handoffOpen}
        onClose={() => setHandoffOpen(false)}
      />
    </div>
  );
}
