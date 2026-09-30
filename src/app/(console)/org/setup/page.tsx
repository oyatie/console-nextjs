"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { StatBar } from "@/components/ui/StatBar";
import { Button } from "@/components/ui/Button";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { OrgEntity, OrgSite, OrgDepartment } from "@/lib/types";
import {
  Building2,
  MapPin,
  Users2,
  Award,
  Plus,
  Edit3,
  CreditCard,
  CheckCircle2,
  X,
  FileCheck,
  ShieldCheck,
  ChevronDown,
  ChevronRight,
  FolderTree,
  Sparkles,
} from "lucide-react";

export default function OrgSetupPage() {
  const orgEntities = useAppStore((s) => s.orgEntities);
  const orgSites = useAppStore((s) => s.orgSites);
  const orgDepartments = useAppStore((s) => s.orgDepartments);
  const employees = useAppStore((s) => s.employees);
  const createOrgEntity = useAppStore((s) => s.createOrgEntity);
  const createOrgSite = useAppStore((s) => s.createOrgSite);
  const createDepartment = useAppStore((s) => s.createDepartment);

  const [activeTab, setActiveTab] = useState<"entities" | "sites" | "depts" | "positions">("entities");
  const [expandedDeptId, setExpandedDeptId] = useState<string | null>(null);

  // 모달 상태
  const [entityModalOpen, setEntityModalOpen] = useState(false);
  const [siteModalOpen, setSiteModalOpen] = useState(false);
  const [deptModalOpen, setDeptModalOpen] = useState(false);

  // 법인 등록 폼 상태
  const [entName, setEntName] = useState("");
  const [entCode, setEntCode] = useState("");
  const [entBizNo, setEntBizNo] = useState("");
  const [entCorpNo, setEntCorpNo] = useState("");
  const [entCeo, setEntCeo] = useState("");
  const [entAddress, setEntAddress] = useState("");
  const [entBank, setEntBank] = useState("신한은행");
  const [entAccount, setEntAccount] = useState("");

  // 사업장 등록 폼 상태
  const [siteName, setSiteName] = useState("");
  const [siteCode, setSiteCode] = useState("");
  const [siteEntityId, setSiteEntityId] = useState(orgEntities[0]?.id || "corp_01");
  const [siteCategory, setSiteCategory] = useState<OrgSite["category"]>("물류센터");
  const [siteAddress, setSiteAddress] = useState("");
  const [siteManager, setSiteManager] = useState("");
  const [safetyManager, setSafetyManager] = useState("");
  const [sitePhone, setSitePhone] = useState("");

  // 부서 등록 폼 상태
  const [deptName, setDeptName] = useState("");
  const [deptCode, setDeptCode] = useState("");
  const [deptEntityId, setDeptEntityId] = useState(orgEntities[0]?.id || "corp_01");
  const [deptDivision, setDeptDivision] = useState("경영총괄본부");
  const [deptManager, setDeptManager] = useState("");
  const [deptCostCenter, setDeptCostCenter] = useState("CC-");

  const handleCreateEntity = (e: React.FormEvent) => {
    e.preventDefault();
    if (!entName.trim()) return;
    createOrgEntity({
      name: entName,
      code: entCode || `OYT-${Math.floor(10 + Math.random() * 89)}`,
      bizNumber: entBizNo || "100-00-00000",
      corpNumber: entCorpNo || "110111-0000000",
      ceoName: entCeo || "대표이사",
      address: entAddress || "서울특별시 강남구",
      mainBank: entBank,
      mainAccount: entAccount || "100-000-000000",
      establishedDate: new Date().toISOString().slice(0, 10),
      status: "active",
    });
    setEntityModalOpen(false);
    setEntName("");
    setEntCode("");
    setEntBizNo("");
    setEntAccount("");
  };

  const handleCreateSite = (e: React.FormEvent) => {
    e.preventDefault();
    if (!siteName.trim()) return;
    createOrgSite({
      name: siteName,
      code: siteCode || `SITE-${Math.floor(10 + Math.random() * 89)}`,
      entityId: siteEntityId,
      category: siteCategory,
      address: siteAddress || "대한민국",
      siteManager: siteManager || "현장총괄소장",
      safetyManager: safetyManager || "안전관리책임자",
      phone: sitePhone || "02-000-0000",
      activeHeadcount: 10,
    });
    setSiteModalOpen(false);
    setSiteName("");
  };

  const handleCreateDept = (e: React.FormEvent) => {
    e.preventDefault();
    if (!deptName.trim()) return;
    createDepartment({
      name: deptName,
      code: deptCode || `D-${Math.floor(100 + Math.random() * 899)}`,
      entityId: deptEntityId,
      division: deptDivision,
      managerName: deptManager || "팀장",
      costCenter: deptCostCenter || "CC-9000",
    });
    setDeptModalOpen(false);
    setDeptName("");
  };

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "산하 법인 (Entities)", value: `${orgEntities.length}개사`, badge: "지주·자회사", badgeTone: "ok" },
          { label: "운영 사업장 (Sites)", value: `${orgSites.length}개소`, badge: "전국 거점", badgeTone: "info" },
          { label: "편성 부서 (Depts)", value: `${orgDepartments.length}개 부서`, subValue: "코스트센터 편제" },
          { label: "재직 총원", value: `${employees.length}명`, subValue: "인사 원장 등재 인원" },
        ]}
      />

      {/* 2. 상단 탭 네비게이션 & 등록 액션 */}
      <div className="flex items-center justify-between flex-wrap gap-2">
        <div className="flex items-center gap-1.5 p-1 rounded-md border border-[var(--border)] bg-[var(--surface)] text-xs">
          <button
            onClick={() => setActiveTab("entities")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "entities" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <Building2 className="w-3.5 h-3.5" />
            <span>법인 관리 ({orgEntities.length})</span>
          </button>
          <button
            onClick={() => setActiveTab("sites")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "sites" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <MapPin className="w-3.5 h-3.5" />
            <span>사업장 · 현장 ({orgSites.length})</span>
          </button>
          <button
            onClick={() => setActiveTab("depts")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "depts" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <Users2 className="w-3.5 h-3.5" />
            <span>부서 · 조직도 ({orgDepartments.length})</span>
          </button>
          <button
            onClick={() => setActiveTab("positions")}
            className={`flex items-center gap-1.5 px-3 py-1.5 rounded font-medium cursor-pointer transition-colors ${
              activeTab === "positions" ? "bg-amber-100 text-amber-900 font-bold shadow-2xs" : "text-[var(--steel)]"
            }`}
          >
            <Award className="w-3.5 h-3.5" />
            <span>직위 · 직급 체계</span>
          </button>
        </div>

        {activeTab === "entities" && (
          <Button
            size="sm"
            variant="brand"
            leftIcon={<Plus className="w-3.5 h-3.5" />}
            onClick={() => setEntityModalOpen(true)}
          >
            신규 법인 설립 등록
          </Button>
        )}
        {activeTab === "sites" && (
          <Button
            size="sm"
            variant="brand"
            leftIcon={<Plus className="w-3.5 h-3.5" />}
            onClick={() => setSiteModalOpen(true)}
          >
            신규 사업장 등록
          </Button>
        )}
        {activeTab === "depts" && (
          <Button
            size="sm"
            variant="brand"
            leftIcon={<Plus className="w-3.5 h-3.5" />}
            onClick={() => setDeptModalOpen(true)}
          >
            신규 부서 편성
          </Button>
        )}
      </div>

      {/* 3. 탭별 메인 뷰 */}
      {activeTab === "entities" && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {orgEntities.map((ent) => (
            <div
              key={ent.id}
              className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 shadow-2xs space-y-3"
            >
              <div className="flex items-start justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-9 h-9 rounded-lg bg-[var(--brand)]/10 text-[var(--brand)] flex items-center justify-center font-bold text-sm">
                    {ent.code.slice(0, 3)}
                  </div>
                  <div>
                    <h3 className="font-bold text-sm text-[var(--ink)]">{ent.name}</h3>
                    <span className="font-mono text-[10px] text-[var(--faint)]">{ent.code} • 설립일: {ent.establishedDate}</span>
                  </div>
                </div>
                <StatusChip label={ent.status === "active" ? "운영중" : "휴지"} tone="ok" size="xs" dot />
              </div>

              <div className="grid grid-cols-2 gap-2 text-xs pt-2 border-t border-[var(--border)]">
                <div>
                  <span className="text-[var(--steel)] block text-[11px]">사업자등록번호</span>
                  <span className="font-mono font-medium text-[var(--ink)]">{ent.bizNumber}</span>
                </div>
                <div>
                  <span className="text-[var(--steel)] block text-[11px]">법인등록번호</span>
                  <span className="font-mono font-medium text-[var(--ink)]">{ent.corpNumber}</span>
                </div>
                <div>
                  <span className="text-[var(--steel)] block text-[11px]">대표자 성명</span>
                  <span className="font-medium text-[var(--ink)]">{ent.ceoName}</span>
                </div>
                <div>
                  <span className="text-[var(--steel)] block text-[11px]">펌뱅킹 모계좌 (출금)</span>
                  <span className="font-mono font-semibold text-[var(--ink)]">{ent.mainBank} {ent.mainAccount}</span>
                </div>
                <div className="col-span-2">
                  <span className="text-[var(--steel)] block text-[11px]">본점 소재지</span>
                  <span className="text-[var(--ink)] truncate block">{ent.address}</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {activeTab === "sites" && (
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden shadow-2xs">
          <table className="w-full text-xs text-left border-collapse">
            <thead>
              <tr className="bg-[var(--muted)] text-[var(--steel)] border-b border-[var(--border)]">
                <th className="px-3.5 py-2.5">사업장 코드</th>
                <th className="px-3.5 py-2.5">사업장명</th>
                <th className="px-3.5 py-2.5">구분</th>
                <th className="px-3.5 py-2.5">소재지 주소</th>
                <th className="px-3.5 py-2.5">현장 총괄 소장</th>
                <th className="px-3.5 py-2.5">안전관리 책임자</th>
                <th className="px-3.5 py-2.5 text-right">상주 인원</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-soft)]">
              {orgSites.map((st) => (
                <tr key={st.id} className="hover:bg-gray-50/70">
                  <td className="px-3.5 py-2.5 font-mono font-semibold text-[var(--brand)]">{st.code}</td>
                  <td className="px-3.5 py-2.5 font-bold text-[var(--ink)]">{st.name}</td>
                  <td className="px-3.5 py-2.5">
                    <StatusChip label={st.category} tone="info" size="xs" />
                  </td>
                  <td className="px-3.5 py-2.5 text-[var(--steel)]">{st.address}</td>
                  <td className="px-3.5 py-2.5 font-medium text-[var(--ink)]">{st.siteManager}</td>
                  <td className="px-3.5 py-2.5 text-emerald-800 flex items-center gap-1">
                    <ShieldCheck className="w-3.5 h-3.5 text-emerald-600" />
                    <span>{st.safetyManager}</span>
                  </td>
                  <td className="px-3.5 py-2.5 text-right font-mono font-bold text-[var(--ink)]">
                    {st.activeHeadcount}명
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {activeTab === "depts" && (
        <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden shadow-2xs">
          <table className="w-full text-xs text-left border-collapse">
            <thead>
              <tr className="bg-[var(--muted)] text-[var(--steel)] border-b border-[var(--border)]">
                <th className="px-3.5 py-2.5">부서 코드</th>
                <th className="w-8 px-2 py-2.5"></th>
                <th className="px-3.5 py-2.5">소속 법인</th>
                <th className="px-3.5 py-2.5">상위 본부</th>
                <th className="px-3.5 py-2.5">부서명</th>
                <th className="px-3.5 py-2.5">부서장 (Lead)</th>
                <th className="px-3.5 py-2.5">코스트센터 (SAP)</th>
                <th className="px-3.5 py-2.5 text-right">소속 인원</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-soft)]">
              {orgDepartments.map((dp) => {
                const ent = orgEntities.find((e) => e.id === dp.entityId);
                const assignedStaff = employees.filter((e) => e.dept === dp.name);
                const count = assignedStaff.length;
                const isExpanded = expandedDeptId === dp.id;

                return (
                  <React.Fragment key={dp.id}>
                    <tr
                      onClick={() => setExpandedDeptId(isExpanded ? null : dp.id)}
                      className={`hover:bg-blue-50/40 cursor-pointer transition-colors ${
                        isExpanded ? "bg-blue-50/20" : ""
                      }`}
                    >
                      <td className="px-2 py-2.5 text-center text-[var(--steel)]">
                        {isExpanded ? (
                          <ChevronDown className="w-3.5 h-3.5 text-blue-600" />
                        ) : (
                          <ChevronRight className="w-3.5 h-3.5 text-gray-400" />
                        )}
                      </td>
                      <td className="px-3.5 py-2.5 font-mono text-[var(--steel)]">{dp.code}</td>
                      <td className="px-3.5 py-2.5 text-[var(--steel)]">{ent?.name || "지주사"}</td>
                      <td className="px-3.5 py-2.5 text-[var(--steel)]">{dp.division}</td>
                      <td className="px-3.5 py-2.5 font-bold text-[var(--ink)]">{dp.name}</td>
                      <td className="px-3.5 py-2.5 font-medium text-[var(--ink)]">{dp.managerName}</td>
                      <td className="px-3.5 py-2.5 font-mono font-medium text-blue-900 bg-blue-50/50 px-2 py-0.5 rounded w-max">
                        {dp.costCenter}
                      </td>
                      <td className="px-3.5 py-2.5 text-right font-mono font-bold text-[var(--ink)]">
                        {count > 0 ? `${count}명` : "편제중"}
                      </td>
                    </tr>
                    {isExpanded && (
                      <tr className="bg-slate-50/80">
                        <td colSpan={8} className="p-3 pl-10">
                          <div className="rounded-lg border border-blue-200 bg-white p-3 space-y-2">
                            <div className="flex items-center justify-between text-xs border-b border-[var(--border-soft)] pb-1.5 font-bold text-[var(--ink)]">
                              <span className="flex items-center gap-1.5">
                                <Users2 className="w-3.5 h-3.5 text-blue-600" />
                                <span>{dp.name} 소속 배속 임직원 현황 ({assignedStaff.length}명)</span>
                              </span>
                              <span className="text-[11px] text-[var(--steel)]">
                                총괄 부서장: <strong>{dp.managerName}</strong> · 코스트센터: <span className="font-mono">{dp.costCenter}</span>
                              </span>
                            </div>
                            {assignedStaff.length === 0 ? (
                              <div className="text-xs text-[var(--steel)] py-2 text-center">
                                현재 본 부서로 공식 인사 발령된 재직 사원이 없습니다.
                              </div>
                            ) : (
                              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2 text-xs">
                                {assignedStaff.map((staff) => (
                                  <div
                                    key={staff.id}
                                    className="p-2 rounded bg-gray-50 border border-[var(--border)] flex items-center justify-between"
                                  >
                                    <div>
                                      <span className="font-bold text-[var(--ink)] block">{staff.name}</span>
                                      <span className="font-mono text-[10px] text-[var(--steel)]">
                                        {staff.code} · {staff.role} ({staff.empType})
                                      </span>
                                    </div>
                                    <div className="text-right font-mono text-[11px] text-blue-800 font-semibold">
                                      {staff.baseSalary.toLocaleString()}원
                                    </div>
                                  </div>
                                ))}
                              </div>
                            )}
                          </div>
                        </td>
                      </tr>
                    )}
                  </React.Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {activeTab === "positions" && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 space-y-3">
            <h3 className="font-bold text-xs text-[var(--ink)] flex items-center gap-1.5">
              <Award className="w-4 h-4 text-amber-600" />
              <span>일반 사무직 · 관리직 직급 체계</span>
            </h3>
            <div className="space-y-2 text-xs">
              {[
                { title: "대표이사 / 회장단", grade: "Executive", tenure: "경영진 선임", role: "경영권 및 최종 의사결정" },
                { title: "총괄본부장 / 전무·상무", grade: "C-Level / VP", tenure: "임원", role: "본부 관할 및 DoA 승인" },
                { title: "실장 / 수석부장", grade: "G5 (Principal)", tenure: "15년차 이상", role: "전략 기획 및 총괄 관리" },
                { title: "팀장 / 책임", grade: "G4 (Lead)", tenure: "10년~14년차", role: "조직 리더 및 1차 전결" },
                { title: "선임 / 과장", grade: "G3 (Senior)", tenure: "6년~9년차", role: "실무 총괄 및 프로젝트 리딩" },
                { title: "주임 / 대리", grade: "G2 (Associate)", tenure: "3년~5년차", role: "단독 실무 수행" },
                { title: "사원", grade: "G1 (Staff)", tenure: "1년~2년차", role: "기초 실무 및 현장 지원" },
              ].map((p, idx) => (
                <div key={idx} className="flex items-center justify-between p-2 rounded bg-[var(--canvas)] border border-[var(--border)]/60">
                  <div>
                    <span className="font-bold text-[var(--ink)]">{p.title}</span>
                    <span className="block text-[11px] text-[var(--steel)]">{p.role}</span>
                  </div>
                  <div className="text-right font-mono text-[11px]">
                    <span className="font-semibold text-amber-900 bg-amber-50 px-1.5 py-0.5 rounded border border-amber-200">{p.grade}</span>
                    <span className="block text-[10px] text-[var(--faint)] mt-0.5">{p.tenure}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>

          <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4 space-y-3">
            <h3 className="font-bold text-xs text-[var(--ink)] flex items-center gap-1.5">
              <Award className="w-4 h-4 text-sky-600" />
              <span>현장 엔지니어 · 기술직 직급 체계</span>
            </h3>
            <div className="space-y-2 text-xs">
              {[
                { title: "기술총괄 소장 (Master)", grade: "T5 (Tech Director)", role: "현장 전 공정 안전 및 가동 총괄" },
                { title: "수석정비기사 (Chief Specialist)", grade: "T4 (Principal Eng)", role: "고난도 유압/전장 특수정비 인가" },
                { title: "책임정비사 (Senior Specialist)", grade: "T3 (Senior Eng)", role: "중장비 분해점검 및 현장 배차" },
                { title: "선임정비사 / 반장 (Lead)", grade: "T2 (Field Lead)", role: "라인 안전검사 및 일일 작업오더 배분" },
                { title: "일반정비기사 / 조원 (Technician)", grade: "T1 (Technician)", role: "예방정비, 오일/소모품 교체 및 운행" },
              ].map((p, idx) => (
                <div key={idx} className="flex items-center justify-between p-2 rounded bg-[var(--canvas)] border border-[var(--border)]/60">
                  <div>
                    <span className="font-bold text-[var(--ink)]">{p.title}</span>
                    <span className="block text-[11px] text-[var(--steel)]">{p.role}</span>
                  </div>
                  <div className="text-right font-mono text-[11px]">
                    <span className="font-semibold text-sky-900 bg-sky-50 px-1.5 py-0.5 rounded border border-sky-200">{p.grade}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* 4. 법인 등록 모달 */}
      {entityModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Building2 className="w-5 h-5 text-[var(--brand)]" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 법인 설립 등록</h3>
              </div>
              <button onClick={() => setEntityModalOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            <form onSubmit={handleCreateEntity} className="space-y-3 text-xs">
              <div>
                <label className="font-semibold block mb-1">법인 상호명 *</label>
                <input
                  type="text"
                  required
                  placeholder="예: (주)오야티 에너지솔루션"
                  value={entName}
                  onChange={(e) => setEntName(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">법인 식별 코드</label>
                  <input
                    type="text"
                    placeholder="OYT-ENG"
                    value={entCode}
                    onChange={(e) => setEntCode(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">대표자 성명</label>
                  <input
                    type="text"
                    placeholder="홍길동"
                    value={entCeo}
                    onChange={(e) => setEntCeo(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">사업자등록번호</label>
                  <input
                    type="text"
                    placeholder="123-45-67890"
                    value={entBizNo}
                    onChange={(e) => setEntBizNo(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">법인등록번호</label>
                  <input
                    type="text"
                    placeholder="110111-1234567"
                    value={entCorpNo}
                    onChange={(e) => setEntCorpNo(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">주거래 은행</label>
                  <select
                    value={entBank}
                    onChange={(e) => setEntBank(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="신한은행">신한은행 (088)</option>
                    <option value="우리은행">우리은행 (020)</option>
                    <option value="국민은행">국민은행 (004)</option>
                    <option value="하나은행">하나은행 (081)</option>
                    <option value="IBK기업은행">IBK기업은행 (003)</option>
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">모계좌 (펌뱅킹 출금용)</label>
                  <input
                    type="text"
                    placeholder="100-032-998231"
                    value={entAccount}
                    onChange={(e) => setEntAccount(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">본점 소재지 주소</label>
                <input
                  type="text"
                  placeholder="서울특별시 강남구 테헤란로 412"
                  value={entAddress}
                  onChange={(e) => setEntAddress(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button size="sm" variant="ghost" onClick={() => setEntityModalOpen(false)}>취소</Button>
                <Button size="sm" variant="brand" type="submit">법인 설립 확정</Button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 5. 사업장 등록 모달 */}
      {siteModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <MapPin className="w-5 h-5 text-sky-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 사업장 / 현장 등록</h3>
              </div>
              <button onClick={() => setSiteModalOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            <form onSubmit={handleCreateSite} className="space-y-3 text-xs">
              <div>
                <label className="font-semibold block mb-1">사업장 명칭 *</label>
                <input
                  type="text"
                  required
                  placeholder="예: 부산 신항 제2물류센터"
                  value={siteName}
                  onChange={(e) => setSiteName(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">소속 법인</label>
                  <select
                    value={siteEntityId}
                    onChange={(e) => setSiteEntityId(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgEntities.map((ent) => (
                      <option key={ent.id} value={ent.id}>{ent.name}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">사업장 분류</label>
                  <select
                    value={siteCategory}
                    onChange={(e) => setSiteCategory(e.target.value as any)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="물류센터">물류센터</option>
                    <option value="항만터미널">항만터미널</option>
                    <option value="정비사업소">정비사업소</option>
                    <option value="제철운영소">제철운영소</option>
                    <option value="본사">본사</option>
                  </select>
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">현장 총괄 소장</label>
                  <input
                    type="text"
                    placeholder="소장 성명"
                    value={siteManager}
                    onChange={(e) => setSiteManager(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">안전관리 책임자 (선임)</label>
                  <input
                    type="text"
                    placeholder="안전기사 성명"
                    value={safetyManager}
                    onChange={(e) => setSafetyManager(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">현장 소재지 주소</label>
                <input
                  type="text"
                  placeholder="상세 주소"
                  value={siteAddress}
                  onChange={(e) => setSiteAddress(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button size="sm" variant="ghost" onClick={() => setSiteModalOpen(false)}>취소</Button>
                <Button size="sm" variant="brand" type="submit">사업장 등록 확정</Button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* 6. 부서 등록 모달 */}
      {deptModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-md rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <Users2 className="w-5 h-5 text-indigo-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 부서 / 조직 편성</h3>
              </div>
              <button onClick={() => setDeptModalOpen(false)} className="text-gray-400 hover:text-gray-600">×</button>
            </div>

            <form onSubmit={handleCreateDept} className="space-y-3 text-xs">
              <div>
                <label className="font-semibold block mb-1">부서명 *</label>
                <input
                  type="text"
                  required
                  placeholder="예: 물류자동화팀"
                  value={deptName}
                  onChange={(e) => setDeptName(e.target.value)}
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">소속 법인</label>
                  <select
                    value={deptEntityId}
                    onChange={(e) => setDeptEntityId(e.target.value)}
                    className="w-full h-8 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    {orgEntities.map((ent) => (
                      <option key={ent.id} value={ent.id}>{ent.name}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">상위 본부</label>
                  <input
                    type="text"
                    placeholder="물류운영본부"
                    value={deptDivision}
                    onChange={(e) => setDeptDivision(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="font-semibold block mb-1">부서장 성명</label>
                  <input
                    type="text"
                    placeholder="팀장 성명"
                    value={deptManager}
                    onChange={(e) => setDeptManager(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  />
                </div>
                <div>
                  <label className="font-semibold block mb-1">코스트센터 코드 (SAP)</label>
                  <input
                    type="text"
                    placeholder="CC-3001"
                    value={deptCostCenter}
                    onChange={(e) => setDeptCostCenter(e.target.value)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              {/* 편제 계층 구조 사전 미리보기 (Hierarchy Construction Preview) */}
              <div className="p-3 rounded-lg border border-blue-200 bg-blue-50/50 space-y-1.5">
                <span className="text-[10px] font-bold text-blue-900 uppercase tracking-wider flex items-center gap-1">
                  <FolderTree className="w-3 h-3 text-blue-700" />
                  <span>조직도 편제 계층 구조 사전 미리보기 (Hierarchy Insertion Preview):</span>
                </span>
                <div className="font-mono text-[11px] space-y-1 bg-white p-2.5 rounded border border-blue-100 text-slate-800">
                  <div className="flex items-center gap-1.5 font-bold text-slate-900">
                    <Building2 className="w-3.5 h-3.5 text-blue-700" />
                    <span>{orgEntities.find((e) => e.id === deptEntityId)?.name || "소속 법인"}</span>
                  </div>
                  <div className="flex items-center gap-1.5 pl-4 text-slate-700">
                    <span>└── 📁 {deptDivision || "상위 본부"}</span>
                  </div>
                  <div className="flex items-center gap-1.5 pl-8 text-emerald-800 font-bold bg-emerald-50 py-0.5 px-1.5 rounded border border-emerald-300">
                    <Sparkles className="w-3 h-3 text-emerald-600" />
                    <span>└── [신규 편제] {deptName || "신규 부서"} ({deptCode || "D-AUTO"})</span>
                  </div>
                </div>
              </div>

              <div className="flex justify-end gap-2 pt-3 border-t border-[var(--border)]">
                <Button size="sm" variant="ghost" onClick={() => setDeptModalOpen(false)}>취소</Button>
                <Button size="sm" variant="brand" type="submit">부서 편성 확정</Button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
}
