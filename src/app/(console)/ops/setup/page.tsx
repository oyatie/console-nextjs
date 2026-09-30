"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Contract, Equipment } from "@/lib/types";
import {
  Briefcase,
  Wrench,
  CalendarCheck,
  Plus,
  AlertCircle,
  Truck,
  CheckCircle2,
  Clock,
  ShieldCheck,
  FileText,
} from "lucide-react";

export default function OpsSetupPage() {
  const contracts = useAppStore((s) => s.contracts);
  const equipment = useAppStore((s) => s.equipment);
  const employees = useAppStore((s) => s.employees);
  const orgSites = useAppStore((s) => s.orgSites);
  const createContract = useAppStore((s) => s.createContract);
  const createEquipment = useAppStore((s) => s.createEquipment);
  const updateEquipmentStatus = useAppStore((s) => s.updateEquipmentStatus);

  const [activeTab, setActiveTab] = useState<"contracts" | "equipment" | "sla">("contracts");

  // 모달 상태
  const [contractModalOpen, setContractModalOpen] = useState(false);
  const [equipmentModalOpen, setEquipmentModalOpen] = useState(false);

  // 계약 등록 폼
  const [contractTitle, setContractTitle] = useState("");
  const [client, setClient] = useState("");
  const [contractSite, setContractSite] = useState(orgSites[0]?.name || "인천 제1물류센터");
  const [contractType, setContractType] = useState<Contract["contractType"]>("도급");
  const [monthlyAmount, setMonthlyAmount] = useState<number>(150000000);
  const [assignedHeadcount, setAssignedHeadcount] = useState<number>(20);
  const [startDate, setStartDate] = useState("2026-07-01");
  const [endDate, setEndDate] = useState("2028-06-30");

  // 장비 등록 폼
  const [eqName, setEqName] = useState("");
  const [eqSite, setEqSite] = useState(orgSites[0]?.name || "인천 제1물류센터");
  const [eqCategory, setEqCategory] = useState<Equipment["category"]>("지게차");
  const [serialNo, setSerialNo] = useState("");
  const [cycleDays, setCycleDays] = useState<number>(90);
  const [assignedEngineer, setAssignedEngineer] = useState(employees[0]?.name || "이준호");

  const handleCreateContract = (e: React.FormEvent) => {
    e.preventDefault();
    if (!contractTitle.trim()) return;
    createContract({
      title: contractTitle,
      client,
      site: contractSite,
      contractType,
      monthlyAmount: Number(monthlyAmount),
      startDate,
      endDate,
      assignedHeadcount: Number(assignedHeadcount),
      status: "유효",
    });
    setContractModalOpen(false);
    setContractTitle("");
    setClient("");
  };

  const handleCreateEquipment = (e: React.FormEvent) => {
    e.preventDefault();
    if (!eqName.trim()) return;
    const today = new Date().toISOString().slice(0, 10);
    const nextDate = new Date(Date.now() + cycleDays * 86400000).toISOString().slice(0, 10);
    createEquipment({
      name: eqName,
      site: eqSite,
      category: eqCategory,
      serialNo: serialNo || "SN-2026-000",
      inspectionCycleDays: Number(cycleDays),
      lastInspectionDate: today,
      nextInspectionDate: nextDate,
      assignedEngineer,
      status: "정상가동",
    });
    setEquipmentModalOpen(false);
    setEqName("");
    setSerialNo("");
  };

  const totalContractMonthly = contracts.reduce((sum, c) => sum + c.monthlyAmount, 0);

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "총 도급/수탁 계약", value: `${contracts.length}건`, subValue: "유효 계약 기준" },
          { label: "월 도급 매출 규모", value: `${Math.round(totalContractMonthly / 100000000)}억원/월`, badge: "도급 원가 산정", badgeTone: "ok" },
          { label: "관리 장비 및 설비", value: `${equipment.length}대`, badge: "현장 가동 자산", badgeTone: "info" },
          {
            label: "정비 중 장비",
            value: `${equipment.filter((e) => e.status === "정비중" || e.status === "점검요망").length}대`,
            badgeTone: "warn",
          },
        ]}
      />

      {/* 2. 상단 탭 및 액션 버튼 */}
      <div className="flex items-center justify-between flex-wrap gap-2">
        <div className="flex items-center gap-1.5 p-1 rounded-md border border-[var(--border)] bg-[var(--surface)] text-xs">
          <button
            onClick={() => setActiveTab("contracts")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "contracts" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <Briefcase className="w-3.5 h-3.5" />
            <span>도급 · 위수탁 계약 ({contracts.length})</span>
          </button>
          <button
            onClick={() => setActiveTab("equipment")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "equipment" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <Truck className="w-3.5 h-3.5" />
            <span>현장 장비 · 설비 자산 ({equipment.length})</span>
          </button>
          <button
            onClick={() => setActiveTab("sla")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "sla" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <Clock className="w-3.5 h-3.5" />
            <span>작업오더 공정 & SLA 템플릿</span>
          </button>
        </div>

        {activeTab === "contracts" && (
          <Button
            size="sm"
            variant="brand"
            leftIcon={<Plus className="w-3.5 h-3.5" />}
            onClick={() => setContractModalOpen(true)}
          >
            신규 도급 계약 등록
          </Button>
        )}
        {activeTab === "equipment" && (
          <Button
            size="sm"
            variant="brand"
            leftIcon={<Plus className="w-3.5 h-3.5" />}
            onClick={() => setEquipmentModalOpen(true)}
          >
            신규 장비 등록
          </Button>
        )}
      </div>

      {/* 3. 탭별 메인 화면 */}
      {activeTab === "contracts" && (
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden shadow-2xs">
          <table className="w-full text-xs text-left border-collapse">
            <thead>
              <tr className="bg-[var(--muted)] text-[var(--steel)] border-b border-[var(--border)]">
                <th className="px-3.5 py-2.5">계약 코드</th>
                <th className="px-3.5 py-2.5">계약명</th>
                <th className="px-3.5 py-2.5">발주처 / 원청</th>
                <th className="px-3.5 py-2.5">배치 현장</th>
                <th className="px-3.5 py-2.5 text-right">월 도급액</th>
                <th className="px-3.5 py-2.5 text-center">투입 인력</th>
                <th className="px-3.5 py-2.5">계약 기간</th>
                <th className="px-3.5 py-2.5 text-center">계약 상태</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-soft)]">
              {contracts.map((c) => (
                <tr key={c.id} className="hover:bg-gray-50/70">
                  <td className="px-3.5 py-2.5">
                    <ObjectLink code={c.code} />
                  </td>
                  <td className="px-3.5 py-2.5 font-bold text-[var(--ink)] max-w-sm truncate">{c.title}</td>
                  <td className="px-3.5 py-2.5 font-medium text-[var(--ink)]">{c.client}</td>
                  <td className="px-3.5 py-2.5 text-[var(--steel)]">{c.site}</td>
                  <td className="px-3.5 py-2.5 text-right font-mono font-bold text-amber-950">
                    {c.monthlyAmount.toLocaleString()}원
                  </td>
                  <td className="px-3.5 py-2.5 text-center font-mono">{c.assignedHeadcount}명</td>
                  <td className="px-3.5 py-2.5 font-mono text-[var(--faint)]">
                    {c.startDate} ~ {c.endDate}
                  </td>
                  <td className="px-3.5 py-2.5 text-center">
                    <StatusChip
                      label={c.status}
                      tone={c.status === "유효" ? "ok" : c.status === "만료임박" ? "warn" : "neutral"}
                      size="xs"
                      dot
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {activeTab === "equipment" && (
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden shadow-2xs">
          <table className="w-full text-xs text-left border-collapse">
            <thead>
              <tr className="bg-[var(--muted)] text-[var(--steel)] border-b border-[var(--border)]">
                <th className="px-3.5 py-2.5">장비 번호</th>
                <th className="px-3.5 py-2.5">장비명 / 규격</th>
                <th className="px-3.5 py-2.5">배치 사업장</th>
                <th className="px-3.5 py-2.5">시리얼 번호</th>
                <th className="px-3.5 py-2.5">점검 주기</th>
                <th className="px-3.5 py-2.5">최근 점검일</th>
                <th className="px-3.5 py-2.5">다음 점검일</th>
                <th className="px-3.5 py-2.5">담당 엔지니어</th>
                <th className="px-3.5 py-2.5 text-center">상태</th>
                <th className="px-3.5 py-2.5 text-center">상태 전환</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-soft)]">
              {equipment.map((eq) => (
                <tr key={eq.id} className="hover:bg-gray-50/70">
                  <td className="px-3.5 py-2.5 font-mono font-bold text-[var(--brand)]">{eq.code}</td>
                  <td className="px-3.5 py-2.5 font-bold text-[var(--ink)]">{eq.name}</td>
                  <td className="px-3.5 py-2.5 text-[var(--steel)]">{eq.site}</td>
                  <td className="px-3.5 py-2.5 font-mono text-[11px] text-[var(--faint)]">{eq.serialNo}</td>
                  <td className="px-3.5 py-2.5 font-mono">{eq.inspectionCycleDays}일</td>
                  <td className="px-3.5 py-2.5 font-mono text-[var(--steel)]">{eq.lastInspectionDate}</td>
                  <td className="px-3.5 py-2.5 font-mono font-bold text-amber-900">{eq.nextInspectionDate}</td>
                  <td className="px-3.5 py-2.5 font-medium">{eq.assignedEngineer}</td>
                  <td className="px-3.5 py-2.5 text-center">
                    <StatusChip
                      label={eq.status}
                      tone={
                        eq.status === "정상가동"
                          ? "ok"
                          : eq.status === "정비중"
                          ? "danger"
                          : "warn"
                      }
                      size="xs"
                      dot
                    />
                  </td>
                  <td className="px-3.5 py-2.5 text-center">
                    <select
                      value={eq.status}
                      onChange={(e) => updateEquipmentStatus(eq.id, e.target.value as any)}
                      className="text-[11px] h-6 px-1 rounded border border-[var(--border)] bg-[var(--surface)] font-medium cursor-pointer"
                    >
                      <option value="정상가동">정상가동</option>
                      <option value="정비중">정비중</option>
                      <option value="점검요망">점검요망</option>
                      <option value="휴지">휴지</option>
                    </select>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {activeTab === "sla" && (
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          <div className="rounded-lg border border-red-200 bg-red-50/40 p-4 space-y-3 shadow-2xs">
            <div className="flex items-center justify-between">
              <span className="font-bold text-red-950 text-sm">긴급 (Urgent SLA)</span>
              <StatusChip label="2시간 내 착수" tone="danger" size="xs" />
            </div>
            <p className="text-xs text-red-900 leading-relaxed">
              항만 선적 크레인 및 메인 물류 라인 중단 시 발령. 즉시 가용 정비기사 강제 배차 및 비상 호출.
            </p>
            <div className="p-2.5 rounded bg-white/80 border border-red-200 text-[11px] space-y-1">
              <div className="font-semibold text-red-950">• 안전 LOTO 즉시 차단</div>
              <div className="font-semibold text-red-950">• 비상 수리자재 긴급 불출</div>
              <div className="font-semibold text-red-950">• 현장 총괄 소장 자동 문자 보고</div>
            </div>
          </div>

          <div className="rounded-lg border border-amber-200 bg-amber-50/40 p-4 space-y-3 shadow-2xs">
            <div className="flex items-center justify-between">
              <span className="font-bold text-amber-950 text-sm">높음 (High SLA)</span>
              <StatusChip label="당일 8시간 내 배차" tone="warn" size="xs" />
            </div>
            <p className="text-xs text-amber-900 leading-relaxed">
              지게차 유압 저하, 부분 작동 이상 등 예비 장비 투입 후 당일 내 정비 완료가 필요한 경우.
            </p>
            <div className="p-2.5 rounded bg-white/80 border border-amber-200 text-[11px] space-y-1">
              <div className="font-semibold text-amber-950">• 정비 2팀 배차 큐 우선 순위</div>
              <div className="font-semibold text-amber-950">• 부품 소요품의 AP-3122 자동 연계</div>
            </div>
          </div>

          <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 space-y-3 shadow-2xs">
            <div className="flex items-center justify-between">
              <span className="font-bold text-[var(--ink)] text-sm">보통 (Standard SLA)</span>
              <StatusChip label="24시간 내 조치" tone="info" size="xs" />
            </div>
            <p className="text-xs text-[var(--steel)] leading-relaxed">
              정기 안전 검사, 오일 교체, 소모품 필터 교체 등 사전 계획된 예방 정비 오더.
            </p>
            <div className="p-2.5 rounded bg-[var(--canvas)] border border-[var(--border)] text-[11px] space-y-1">
              <div className="font-semibold text-[var(--ink)]">• 정기 순회 점검표 작성</div>
              <div className="font-semibold text-[var(--ink)]">• 주간 정비 리포트 자동 집계</div>
            </div>
          </div>
        </div>
      )}

      {/* 4. 계약 등록 모달 */}
      {contractModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-lg rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Briefcase className="w-5 h-5 text-amber-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 도급 및 위수탁 계약 체결 등록</h3>
              </div>
              <button onClick={() => setContractModalOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            <form onSubmit={handleCreateContract} className="space-y-3 text-xs">
              <div>
                <label className="font-semibold block mb-1">도급 계약명 *</label>
                <input
                  type="text"
                  required
                  placeholder="예: 평택항 제2부두 하역 및 장비 운영 도급"
                  value={contractTitle}
                  onChange={(e) => setContractTitle(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">발주처 / 원청사 *</label>
                  <input
                    type="text"
                    required
                    placeholder="현대글로비스 / 한국항만물류협회"
                    value={client}
                    onChange={(e) => setClient(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">수행 사업장</label>
                  <select
                    value={contractSite}
                    onChange={(e) => setContractSite(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgSites.map((st) => (
                      <option key={st.id} value={st.name}>{st.name}</option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-3 gap-2">
                <div>
                  <label className="font-semibold block mb-1">계약 형태</label>
                  <select
                    value={contractType}
                    onChange={(e) => setContractType(e.target.value as any)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="도급">도급</option>
                    <option value="위수탁">위수탁</option>
                    <option value="정비용역">정비용역</option>
                    <option value="화물운송">화물운송</option>
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">월 도급액 (원)</label>
                  <input
                    type="number"
                    value={monthlyAmount}
                    onChange={(e) => setMonthlyAmount(Number(e.target.value))}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono font-bold"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">투입 인력 기준 (명)</label>
                  <input
                    type="number"
                    value={assignedHeadcount}
                    onChange={(e) => setAssignedHeadcount(Number(e.target.value))}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono font-bold"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">계약 시작일</label>
                  <input
                    type="date"
                    value={startDate}
                    onChange={(e) => setStartDate(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">계약 만료일</label>
                  <input
                    type="date"
                    value={endDate}
                    onChange={(e) => setEndDate(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button size="sm" variant="ghost" onClick={() => setContractModalOpen(false)}>취소</Button>
                <Button size="sm" variant="brand" type="submit">계약 등록 확정</Button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 5. 장비 등록 모달 */}
      {equipmentModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Truck className="w-5 h-5 text-sky-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 현장 장비 및 설비 등록</h3>
              </div>
              <button onClick={() => setEquipmentModalOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            <form onSubmit={handleCreateEquipment} className="space-y-3 text-xs">
              <div>
                <label className="font-semibold block mb-1">장비명 / 기종 규격 *</label>
                <input
                  type="text"
                  required
                  placeholder="예: 두산 7톤 전동지게차 (B70X)"
                  value={eqName}
                  onChange={(e) => setEqName(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">배치 사업장</label>
                  <select
                    value={eqSite}
                    onChange={(e) => setEqSite(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgSites.map((st) => (
                      <option key={st.id} value={st.name}>{st.name}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">장비 분류</label>
                  <select
                    value={eqCategory}
                    onChange={(e) => setEqCategory(e.target.value as any)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="지게차">지게차</option>
                    <option value="항만트랙터">항만트랙터</option>
                    <option value="크레인">크레인</option>
                    <option value="특수정비차량">특수정비차량</option>
                    <option value="공조설비">공조설비</option>
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">차대번호 / 시리얼</label>
                  <input
                    type="text"
                    placeholder="DOOSAN-B70X-2026"
                    value={serialNo}
                    onChange={(e) => setSerialNo(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">정기 안전점검 주기 (일)</label>
                  <input
                    type="number"
                    value={cycleDays}
                    onChange={(e) => setCycleDays(Number(e.target.value))}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">담당 엔지니어</label>
                <select
                  value={assignedEngineer}
                  onChange={(e) => setAssignedEngineer(e.target.value)}
                  className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                >
                  {employees.map((emp) => (
                    <option key={emp.id} value={emp.name}>{emp.name} ({emp.dept} · {emp.role})</option>
                  ))}
                </select>
              </div>

              <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button size="sm" variant="ghost" onClick={() => setEquipmentModalOpen(false)}>취소</Button>
                <Button size="sm" variant="brand" type="submit">장비 등록 확정</Button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}
