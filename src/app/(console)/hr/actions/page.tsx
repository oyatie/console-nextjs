"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { PersonnelAction, PersonnelActionType, Employee } from "@/lib/types";
import {
  Award,
  Users,
  TrendingUp,
  FileCheck2,
  Plus,
  ArrowRight,
  ShieldCheck,
  CheckCircle2,
  Printer,
  X,
  FileText,
} from "lucide-react";

export default function HrActionsPage() {
  const personnelActions = useAppStore((s) => s.personnelActions);
  const employees = useAppStore((s) => s.employees);
  const orgSites = useAppStore((s) => s.orgSites);
  const orgDepartments = useAppStore((s) => s.orgDepartments);
  const processPersonnelAction = useAppStore((s) => s.processPersonnelAction);

  const [activeFilter, setActiveFilter] = useState<"all" | "promo" | "transfer" | "salary" | "leave">("all");
  const [composerOpen, setComposerOpen] = useState(false);
  const [selectedAction, setSelectedAction] = useState<PersonnelAction | null>(personnelActions[0] || null);
  const [certModalOpen, setCertModalOpen] = useState(false);

  // 신규 발령 폼 상태
  const [targetEmpId, setTargetEmpId] = useState(employees[0]?.id || "");
  const [actionType, setActionType] = useState<PersonnelActionType>("승진");
  const [effectiveDate, setEffectiveDate] = useState("2026-08-01");
  const [reason, setReason] = useState("");

  const targetEmp = employees.find((e) => e.id === targetEmpId) || employees[0];

  // 변경할 속성들
  const [newDept, setNewDept] = useState(targetEmp?.dept || "경영기획실");
  const [newRole, setNewRole] = useState(targetEmp?.role || "팀장");
  const [newSite, setNewSite] = useState(targetEmp?.site || "서울 본사 타워");
  const [newSalary, setNewSalary] = useState<number>(targetEmp?.baseSalary ? targetEmp.baseSalary + 500000 : 4000000);
  const [newStatus, setNewStatus] = useState<Employee["status"]>("재직");

  const handleSelectEmp = (id: string) => {
    setTargetEmpId(id);
    const emp = employees.find((e) => e.id === id);
    if (emp) {
      setNewDept(emp.dept);
      setNewRole(emp.role);
      setNewSite(emp.site);
      setNewSalary(emp.baseSalary);
      setNewStatus(emp.status);
    }
  };

  const handleCreateAction = (e: React.FormEvent) => {
    e.preventDefault();
    if (!targetEmpId) return;

    const action = processPersonnelAction({
      employeeId: targetEmpId,
      actionType,
      effectiveDate,
      reason: reason || `${actionType}에 따른 정기 인사발령`,
      updates: {
        dept: newDept,
        role: newRole,
        site: newSite,
        baseSalary: Number(newSalary),
        status: newStatus,
      },
    });

    setComposerOpen(false);
    setSelectedAction(action);
    setCertModalOpen(true);
    setReason("");
  };

  const filteredActions = personnelActions.filter((pa) => {
    if (activeFilter === "promo") return pa.actionType === "승진" || pa.actionType === "직책임명";
    if (activeFilter === "transfer") return pa.actionType === "부서전보";
    if (activeFilter === "salary") return pa.actionType === "연봉계약갱신";
    if (activeFilter === "leave") return pa.actionType === "휴직" || pa.actionType === "복직" || pa.actionType === "퇴직";
    return true;
  });

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "총 인사명령 발령", value: `${personnelActions.length}건`, badge: "인사위원회", badgeTone: "ok" },
          {
            label: "당월 발령 건수",
            value: `${personnelActions.filter((p) => p.effectiveDate.startsWith("2026-07") || p.effectiveDate.startsWith("2026-08")).length}건`,
            badge: "효력 발생중",
            badgeTone: "info",
          },
          {
            label: "승진 및 직책 보임",
            value: `${personnelActions.filter((p) => p.actionType === "승진" || p.actionType === "직책임명").length}명`,
            badgeTone: "ok",
          },
          {
            label: "부서 전보 및 현장배치",
            value: `${personnelActions.filter((p) => p.actionType === "부서전보").length}명`,
            badgeTone: "warn",
          },
        ]}
      />

      {/* 2. 상단 액션 및 필터 바 */}
      <div className="flex items-center justify-between flex-wrap gap-2">
        <div className="flex items-center gap-1.5 p-1 rounded-md border border-[var(--border)] bg-[var(--surface)] text-xs">
          <button
            onClick={() => setActiveFilter("all")}
            className={`px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeFilter === "all" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            전체 명령 ({personnelActions.length})
          </button>
          <button
            onClick={() => setActiveFilter("promo")}
            className={`px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeFilter === "promo" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            승진 · 직책
          </button>
          <button
            onClick={() => setActiveFilter("transfer")}
            className={`px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeFilter === "transfer" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            부서 전보
          </button>
          <button
            onClick={() => setActiveFilter("salary")}
            className={`px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeFilter === "salary" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            급여 계약 갱신
          </button>
          <button
            onClick={() => setActiveFilter("leave")}
            className={`px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeFilter === "leave" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            휴직 · 복직 · 퇴직
          </button>
        </div>

        <Button
          size="sm"
          variant="brand"
          leftIcon={<Plus className="w-3.5 h-3.5" />}
          onClick={() => setComposerOpen(true)}
        >
          신규 인사명령 발령
        </Button>
      </div>

      {/* 3. 인사명령 발령 대장 테이블 */}
      <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden shadow-2xs">
        <table className="w-full text-xs text-left border-collapse">
          <thead>
            <tr className="bg-[var(--muted)] text-[var(--steel)] border-b border-[var(--border)]">
              <th className="px-3.5 py-2.5">명령 번호</th>
              <th className="px-3.5 py-2.5">개체 코드</th>
              <th className="px-3.5 py-2.5">발령 구분</th>
              <th className="px-3.5 py-2.5">대상 사원</th>
              <th className="px-3.5 py-2.5">발령 효력일</th>
              <th className="px-3.5 py-2.5">발령 전 (기존 상태)</th>
              <th className="px-3.5 py-2.5">발령 후 (변경 사항)</th>
              <th className="px-3.5 py-2.5">발령 사유</th>
              <th className="px-3.5 py-2.5 text-center">공식 사령장</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-[var(--border-soft)]">
            {filteredActions.map((pa) => (
              <tr key={pa.id} className="hover:bg-gray-50/70">
                <td className="px-3.5 py-2.5 font-mono font-bold text-[var(--ink)]">{pa.orderNumber}</td>
                <td className="px-3.5 py-2.5">
                  <ObjectLink code={pa.code} />
                </td>
                <td className="px-3.5 py-2.5">
                  <StatusChip
                    label={pa.actionType}
                    tone={
                      pa.actionType === "승진"
                        ? "ok"
                        : pa.actionType === "직책임명"
                        ? "brand"
                        : pa.actionType === "부서전보"
                        ? "info"
                        : pa.actionType === "퇴직"
                        ? "danger"
                        : "warn"
                    }
                    size="xs"
                  />
                </td>
                <td className="px-3.5 py-2.5 font-bold text-[var(--ink)]">{pa.employeeName}</td>
                <td className="px-3.5 py-2.5 font-mono text-[var(--faint)]">{pa.effectiveDate}</td>
                <td className="px-3.5 py-2.5 text-[var(--steel)]">
                  {pa.prevDetails.dept} · {pa.prevDetails.role}
                  {pa.prevDetails.baseSalary && (
                    <span className="block font-mono text-[10px] text-[var(--faint)]">
                      {Math.round(pa.prevDetails.baseSalary / 10000)}만원
                    </span>
                  )}
                </td>
                <td className="px-3.5 py-2.5 font-semibold text-amber-900">
                  {pa.newDetails.dept || pa.prevDetails.dept} · {pa.newDetails.role || pa.prevDetails.role}
                  {pa.newDetails.baseSalary && (
                    <span className="block font-mono text-[10px] text-amber-700">
                      {Math.round(pa.newDetails.baseSalary / 10000)}만원
                    </span>
                  )}
                </td>
                <td className="px-3.5 py-2.5 text-[var(--steel)] truncate max-w-xs">{pa.reason}</td>
                <td className="px-3.5 py-2.5 text-center">
                  <Button
                    size="xs"
                    variant="secondary"
                    leftIcon={<FileText className="w-3 h-3" />}
                    onClick={() => {
                      setSelectedAction(pa);
                      setCertModalOpen(true);
                    }}
                  >
                    사령장 열람
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* 4. 신규 인사명령 발령 모달 */}
      {composerOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Award className="w-5 h-5 text-amber-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 임직원 인사명령 발령</h3>
              </div>
              <button onClick={() => setComposerOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            <form onSubmit={handleCreateAction} className="space-y-3 text-xs">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">대상 사원 선택 *</label>
                  <select
                    value={targetEmpId}
                    onChange={(e) => handleSelectEmp(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {employees.map((emp) => (
                      <option key={emp.id} value={emp.id}>
                        {emp.name} ({emp.dept} · {emp.role})
                      </option>
                    ))}
                  </select>
                </div>

                <div>
                  <label className="font-semibold block mb-1">발령 구분 (Action Type) *</label>
                  <select
                    value={actionType}
                    onChange={(e) => setActionType(e.target.value as any)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="승진">승진 (Promotion)</option>
                    <option value="직책임명">직책임명 (Role Assignment)</option>
                    <option value="부서전보">부서전보 (Transfer)</option>
                    <option value="연봉계약갱신">연봉계약갱신 (Salary Revision)</option>
                    <option value="휴직">휴직 (Leave of Absence)</option>
                    <option value="복직">복직 (Return from Leave)</option>
                    <option value="퇴직">퇴직 (Termination)</option>
                  </select>
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">발령 효력 발생일 *</label>
                <input
                  type="date"
                  required
                  value={effectiveDate}
                  onChange={(e) => setEffectiveDate(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                />
              </div>

              <div className="p-3 rounded-lg border border-[var(--border)] bg-[var(--canvas)] space-y-2.5">
                <span className="font-bold text-[11px] text-[var(--ink)] block">발령 후 변경 사항 입력</span>
                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="text-[10px] text-[var(--steel)] block mb-0.5">발령 부서</label>
                    <input
                      type="text"
                      value={newDept}
                      onChange={(e) => setNewDept(e.target.value)}
                      className="w-full h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                    />
                  </div>
                  <div>
                    <label className="text-[10px] text-[var(--steel)] block mb-0.5">발령 직책 / 호칭</label>
                    <input
                      type="text"
                      value={newRole}
                      onChange={(e) => setNewRole(e.target.value)}
                      className="w-full h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                    />
                  </div>
                </div>

                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="text-[10px] text-[var(--steel)] block mb-0.5">배치 사업장</label>
                    <select
                      value={newSite}
                      onChange={(e) => setNewSite(e.target.value)}
                      className="w-full h-7 px-1.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                    >
                      {orgSites.map((s) => (
                        <option key={s.id} value={s.name}>{s.name}</option>
                      ))}
                    </select>
                  </div>
                  <div>
                    <label className="text-[10px] text-[var(--steel)] block mb-0.5">월 기본급 (원)</label>
                    <input
                      type="number"
                      value={newSalary}
                      onChange={(e) => setNewSalary(Number(e.target.value))}
                      className="w-full h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                    />
                  </div>
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">인사명령 사유 및 인사위원회 의결내용</label>
                <textarea
                  rows={2}
                  value={reason}
                  onChange={(e) => setReason(e.target.value)}
                  placeholder="예: 2026 하반기 정기 인사평가 우수 및 조직 개편에 따른 승진 발령"
                  className="w-full p-2.5 rounded border border-[var(--border)] bg-[var(--surface)] leading-relaxed"
                />
              </div>

              <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button size="sm" variant="ghost" onClick={() => setComposerOpen(false)}>취소</Button>
                <Button size="sm" variant="brand" type="submit">인사명령 정식 발령</Button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 5. 공식 인사명령 사령장 (Appointment Notice Certificate Modal) */}
      {certModalOpen && selectedAction && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Award className="w-5 h-5 text-amber-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">공식 인사명령 사령장 (Appointment Order)</h3>
              </div>
              <button onClick={() => setCertModalOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            {/* 정통 기업 사령장 양식 디자인 */}
            <div className="rounded-lg border-2 border-double border-amber-300 bg-amber-50/20 p-6 text-center space-y-4 text-xs font-serif">
              <div className="space-y-1">
                <div className="text-[11px] font-mono text-[var(--steel)] tracking-widest">{selectedAction.orderNumber}</div>
                <h2 className="text-lg font-black text-[var(--ink)] tracking-wider">인 사 발 령 사 령 장</h2>
              </div>

              <div className="py-2 border-y border-amber-200/60 font-mono text-left space-y-1 text-[11px]">
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">소속:</span>
                  <span className="font-bold text-[var(--ink)]">{selectedAction.prevDetails.dept}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">성명:</span>
                  <span className="font-bold text-[var(--ink)]">{selectedAction.employeeName}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-[var(--steel)]">발령 구분:</span>
                  <span className="font-bold text-amber-900">{selectedAction.actionType}</span>
                </div>
              </div>

              <div className="py-4 text-sm font-bold text-[var(--ink)] leading-relaxed">
                "귀하를 {selectedAction.effectiveDate}일부로<br />
                <span className="text-amber-800 underline decoration-amber-400 underline-offset-4">
                  {selectedAction.newDetails.dept || selectedAction.prevDetails.dept}{" "}
                  {selectedAction.newDetails.role || selectedAction.prevDetails.role}
                </span>
                에 임함."
              </div>

              <div className="text-[11px] text-[var(--steel)] font-sans leading-relaxed">
                사유: {selectedAction.reason}
              </div>

              <div className="pt-3 border-t border-amber-200/60 space-y-1">
                <div className="font-sans font-bold text-[var(--ink)]">주식회사 오야티 대표이사 최 원 석</div>
                <div className="text-[10px] text-[var(--faint)] font-mono">인사위원회 의결 및 전자서명 직인 날인 완료</div>
              </div>
            </div>

            <div className="flex items-center justify-between pt-2">
              <span className="text-[10px] font-mono text-emerald-700 flex items-center gap-1">
                <CheckCircle2 className="w-3.5 h-3.5" />
                <span>WORM 감사 원장에 영구 보존 기록됨</span>
              </span>
              <Button size="sm" variant="secondary" onClick={() => setCertModalOpen(false)}>
                닫기
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
