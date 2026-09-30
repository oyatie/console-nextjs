"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { DataTable, Column } from "@/components/ui/DataTable";
import { StatusChip } from "@/components/ui/StatusChip";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Button } from "@/components/ui/Button";
import { StatBar } from "@/components/ui/StatBar";
import { ApprovalDoc, ApprovalLineItem } from "@/lib/types";
import { CapacitySignaturePicker } from "@/components/gov/CapacitySignaturePicker";
import { ModuleScopeFilter, ModuleScopeState } from "@/components/gov/ModuleScopeFilter";
import {
  FileText,
  Plus,
  CheckCircle2,
  XCircle,
  Clock,
  KeyRound,
  ShieldCheck,
  FileCheck,
  Send,
  Trash2,
  Receipt,
  Layers,
  Fingerprint,
} from "lucide-react";

export default function ApprovalsPage() {
  const approvals = useAppStore((s) => s.approvals);
  const createApprovalDraft = useAppStore((s) => s.createApprovalDraft);
  const submitApproval = useAppStore((s) => s.submitApproval);
  const approveDoc = useAppStore((s) => s.approveDoc);
  const rejectDoc = useAppStore((s) => s.rejectDoc);
  const closeDoc = useAppStore((s) => s.closeDoc);
  const viewAsRole = useAppStore((s) => s.viewAsRole);

  const [activeTab, setActiveTab] = useState<"all" | "pending" | "draft" | "approved">("pending");
  const [selectedDocId, setSelectedDocId] = useState<string | null>(approvals[0]?.id || null);

  // 현재 선택된 문서 (store에서 최신 상태 구독)
  const selectedDoc = approvals.find((a) => a.id === selectedDocId) || approvals[0] || null;

  const employees = useAppStore((s) => s.employees);

  // 모달 상태
  const [composerOpen, setComposerOpen] = useState(false);
  const [capacitySignModalOpen, setCapacitySignModalOpen] = useState(false);
  const [capacityTargetDoc, setCapacityTargetDoc] = useState<ApprovalDoc | null>(null);
  const [passkeyModalOpen, setPasskeyModalOpen] = useState(false);
  const [passkeyTargetDoc, setPasskeyTargetDoc] = useState<ApprovalDoc | null>(null);
  const [passkeyCeremonyStep, setPasskeyCeremonyStep] = useState<"challenge" | "touch" | "verified">("challenge");

  // Active approver persona derived from viewAsRole or employee list
  const activeApprover = employees.find((e) => e.role === viewAsRole || e.dept.includes("인사")) || employees[0];

  // 기안 폼 상태
  const [formCategory, setFormCategory] = useState<ApprovalDoc["category"]>("연장근로");
  const [formTitle, setFormTitle] = useState("");
  const [formContent, setFormContent] = useState("");
  const [formLinkedCode, setFormLinkedCode] = useState("C-207");
  const [formLineItems, setFormLineItems] = useState<Array<{ name: string; qty: number; unitPrice: number; totalPrice: number }>>([
    { name: "현장 긴급 정비 부품", qty: 1, unitPrice: 250000, totalPrice: 250000 },
  ]);

  // 품목 라인 합계 계산
  const computedTotalAmount = formLineItems.reduce((sum, li) => sum + (li.totalPrice || 0), 0);

  const addLineItem = () => {
    setFormLineItems((prev) => [
      ...prev,
      { name: "", qty: 1, unitPrice: 0, totalPrice: 0 },
    ]);
  };

  const updateLineItem = (index: number, field: "name" | "qty" | "unitPrice", val: string | number) => {
    setFormLineItems((prev) => {
      const copy = [...prev];
      const target = { ...copy[index] };
      if (field === "name") target.name = String(val);
      if (field === "qty") {
        target.qty = Math.max(1, Number(val));
        target.totalPrice = target.qty * target.unitPrice;
      }
      if (field === "unitPrice") {
        target.unitPrice = Math.max(0, Number(val));
        target.totalPrice = target.qty * target.unitPrice;
      }
      copy[index] = target;
      return copy;
    });
  };

  const removeLineItem = (index: number) => {
    setFormLineItems((prev) => prev.filter((_, i) => i !== index));
  };

  // 모듈 관할 인가 범위 필터 상태
  const [scopeFilter, setScopeFilter] = useState<ModuleScopeState>({
    entityId: "all",
    siteId: "all",
    deptId: "all",
  });

  // 필터링
  const filteredDocs = React.useMemo(() => {
    return approvals.filter((doc) => {
      if (activeTab === "pending" && doc.status !== "결재대기") return false;
      if (activeTab === "draft" && doc.status !== "초안") return false;
      if (activeTab === "approved" && !(doc.status === "승인완료" || doc.status === "종결")) return false;

      if (scopeFilter.deptId !== "all" && !doc.drafterDept.includes(scopeFilter.deptId)) return false;
      if (scopeFilter.entityId !== "all") {
        const drafterEmp = employees.find((e) => e.id === doc.drafterId || e.name === doc.drafterName);
        if (drafterEmp && drafterEmp.entity !== scopeFilter.entityId) return false;
      }
      return true;
    });
  }, [approvals, activeTab, scopeFilter, employees]);

  const handleCreateDraft = (asSubmit: boolean) => {
    if (!formTitle.trim()) return;

    const formattedLines: ApprovalLineItem[] = formLineItems
      .filter((li) => li.name.trim())
      .map((li, i) => ({
        id: `li_${Date.now()}_${i}`,
        name: li.name,
        item: li.name,
        qty: li.qty,
        unitPrice: li.unitPrice,
        totalPrice: li.totalPrice,
        amount: li.totalPrice,
      }));

    const newDoc = createApprovalDraft({
      title: formTitle,
      category: formCategory,
      drafterId: "current_user",
      drafterName: "박지영 수석",
      drafterDept: "(주)오야티 코퍼레이션 인사노무팀",
      content: formContent || "상세 품의 내용이 기재되었습니다.",
      doaAmount: computedTotalAmount > 0 ? computedTotalAmount : undefined,
      linkedObjects: formLinkedCode ? [formLinkedCode] : [],
      lineItems: formattedLines.length > 0 ? formattedLines : undefined,
    });

    if (asSubmit) {
      submitApproval(newDoc.id);
    }

    setComposerOpen(false);
    setFormTitle("");
    setFormContent("");
    setFormLineItems([{ name: "현장 긴급 정비 부품", qty: 1, unitPrice: 250000, totalPrice: 250000 }]);
    setSelectedDocId(newDoc.id);
  };

  const handleTriggerApproval = (doc: ApprovalDoc) => {
    setCapacityTargetDoc(doc);
    setCapacitySignModalOpen(true);
  };

  const handleCapacitySign = (signature: {
    actingCapacityRole: string;
    actingCapacityName: string;
    authorizingGrantId?: string;
    passkeyVerified: boolean;
    comment: string;
  }) => {
    if (!capacityTargetDoc) return;
    approveDoc(
      capacityTargetDoc.id,
      `${activeApprover?.name || viewAsRole}`,
      signature.comment,
      {
        actingCapacityRole: signature.actingCapacityRole,
        actingCapacityName: signature.actingCapacityName,
        authorizingGrantId: signature.authorizingGrantId,
        passkeyVerified: signature.passkeyVerified,
      }
    );
    setCapacitySignModalOpen(false);
  };

  const executePasskeyVerification = () => {
    setPasskeyCeremonyStep("touch");
    setTimeout(() => {
      setPasskeyCeremonyStep("verified");
      setTimeout(() => {
        if (passkeyTargetDoc) {
          approveDoc(passkeyTargetDoc.id, `${viewAsRole} (Passkey 인증)`, "FIDO2 생체인증 전자결재 승인", {
            actingCapacityRole: "role-super-admin",
            actingCapacityName: "FIDO2 생체인증 전자결재",
            passkeyVerified: true,
          });
        }
        setPasskeyModalOpen(false);
      }, 700);
    }, 1200);
  };

  const columns: Column<ApprovalDoc>[] = [
    {
      key: "code",
      header: "문서 번호",
      width: "110px",
      render: (row) => <ObjectLink code={row.code} />,
    },
    {
      key: "category",
      header: "분류",
      width: "90px",
      render: (row) => <StatusChip label={row.category} tone="info" size="xs" />,
    },
    {
      key: "title",
      header: "기안 제목",
      render: (row) => (
        <span className="font-semibold text-[var(--ink)] truncate block max-w-sm">
          {row.title}
        </span>
      ),
    },
    {
      key: "drafterName",
      header: "기안자",
      width: "100px",
      render: (row) => <span className="text-[var(--steel)]">{row.drafterName}</span>,
    },
    {
      key: "createdAt",
      header: "기안 시각",
      width: "130px",
      render: (row) => <span className="font-mono text-[11px] text-[var(--faint)]">{row.createdAt}</span>,
    },
    {
      key: "status",
      header: "진행 상태",
      width: "90px",
      render: (row) => <StatusChip label={row.status} size="xs" dot />,
    },
  ];

  return (
    <div className="space-y-5">
      {/* 1. 상단 통계 바 */}
      <StatBar
        items={[
          { label: "전체 기안 대장", value: `${approvals.length}건` },
          {
            label: "결재 대기",
            value: `${approvals.filter((a) => a.status === "결재대기").length}건`,
            badge: "심사 대상",
            badgeTone: "warn",
          },
          {
            label: "예비 저장 (파킹)",
            value: `${approvals.filter((a) => a.status === "초안").length}건`,
            badge: "작성중",
            badgeTone: "info",
          },
          {
            label: "승인 완료",
            value: `${approvals.filter((a) => a.status === "승인완료").length}건`,
            badgeTone: "ok",
          },
        ]}
      />

      {/* 2. 메인 워크스페이스 (2열 Master-Detail Layout) */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
        {/* 좌측: 결재 목록 (7열) */}
        <div className="lg:col-span-7 space-y-3">
          <div className="flex items-center justify-between flex-wrap gap-2">
            <div className="flex items-center gap-1.5 p-1 rounded-md border border-[var(--border)] bg-[var(--surface)] text-xs">
              <button
                onClick={() => setActiveTab("pending")}
                className={`px-3 py-1 rounded font-medium cursor-pointer transition-colors ${
                  activeTab === "pending" ? "bg-amber-100 text-amber-900 font-bold" : "text-[var(--steel)]"
                }`}
              >
                결재 대기 ({approvals.filter((a) => a.status === "결재대기").length})
              </button>
              <button
                onClick={() => setActiveTab("draft")}
                className={`px-3 py-1 rounded font-medium cursor-pointer transition-colors ${
                  activeTab === "draft" ? "bg-amber-100 text-amber-900 font-bold" : "text-[var(--steel)]"
                }`}
              >
                예비 파킹 ({approvals.filter((a) => a.status === "초안").length})
              </button>
              <button
                onClick={() => setActiveTab("all")}
                className={`px-3 py-1 rounded font-medium cursor-pointer transition-colors ${
                  activeTab === "all" ? "bg-amber-100 text-amber-900 font-bold" : "text-[var(--steel)]"
                }`}
              >
                전체 대장 ({approvals.length})
              </button>
            </div>

            <Button
              size="sm"
              variant="brand"
              leftIcon={<Plus className="w-3.5 h-3.5" />}
              onClick={() => setComposerOpen(true)}
            >
              새 기안문서 작성
            </Button>
          </div>

          {/* 모듈 레벨 관할 인가 범위 필터 */}
          <ModuleScopeFilter
            value={scopeFilter}
            onChange={setScopeFilter}
            filteredCount={filteredDocs.length}
            totalCount={approvals.length}
            showSite={false}
          />

          <DataTable
            columns={columns}
            data={filteredDocs}
            keyExtractor={(item) => item.id}
            onRowClick={(item) => setSelectedDocId(item.id)}
            searchPlaceholder="기안 제목, 문서번호, 기안자 검색..."
            searchFilter={(item, q) =>
              item.title.toLowerCase().includes(q.toLowerCase()) ||
              item.code.toLowerCase().includes(q.toLowerCase()) ||
              item.drafterName.toLowerCase().includes(q.toLowerCase())
            }
          />
        </div>

        {/* 우측: 선택된 결재 문서 상세 뷰어 & 액션 콘솔 (5열) */}
        <div className="lg:col-span-5">
          {selectedDoc ? (
            <div className="rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs p-4 space-y-4">
              {/* 헤더 정보 */}
              <div className="border-b border-[var(--border)] pb-3 space-y-2">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <ObjectLink code={selectedDoc.code} />
                    <StatusChip label={selectedDoc.category} tone="info" size="xs" />
                  </div>
                  <StatusChip label={selectedDoc.status} dot />
                </div>
                <h3 className="text-sm font-bold text-[var(--ink)]">{selectedDoc.title}</h3>
                <div className="text-[11px] text-[var(--steel)] flex items-center justify-between">
                  <span>기안자: {selectedDoc.drafterName} ({selectedDoc.drafterDept})</span>
                  <span className="font-mono">{selectedDoc.createdAt}</span>
                </div>
              </div>

              {/* 금액 및 DoA 한도 */}
              {selectedDoc.doaAmount && (
                <div className="p-2.5 rounded border border-blue-200 bg-blue-50/50 flex items-center justify-between text-xs">
                  <span className="text-blue-900 font-medium">품의 총 금액 (DoA 판정)</span>
                  <span className="font-mono font-bold text-sm text-blue-950">
                    {selectedDoc.doaAmount.toLocaleString()}원
                  </span>
                </div>
              )}

              {/* 기안 본문 */}
              <div className="space-y-1">
                <div className="text-[11px] font-semibold text-[var(--steel)]">기안 내용</div>
                <div className="p-3 rounded border border-[var(--border)] bg-gray-50/50 text-xs text-[var(--ink)] leading-relaxed whitespace-pre-wrap min-h-[70px]">
                  {selectedDoc.content}
                </div>
              </div>

              {/* 품목별 지출·청구 명세서 (Itemized Expense Line Items) */}
              {selectedDoc.lineItems && selectedDoc.lineItems.length > 0 && (
                <div className="space-y-1.5">
                  <div className="flex items-center gap-1.5 text-[11px] font-semibold text-[var(--ink)]">
                    <Receipt className="w-3.5 h-3.5 text-blue-600" />
                    <span>청구 품목 세부 명세 ({selectedDoc.lineItems.length}개 항목)</span>
                  </div>
                  <div className="border border-[var(--border)] rounded-md overflow-hidden text-xs">
                    <table className="w-full text-left border-collapse">
                      <thead className="bg-[var(--muted)] text-[var(--steel)] border-b border-[var(--border)] text-[11px]">
                        <tr>
                          <th className="px-2.5 py-1.5">품목 / 규격</th>
                          <th className="px-2 py-1.5 text-right w-12">수량</th>
                          <th className="px-2 py-1.5 text-right w-20">단가</th>
                          <th className="px-2.5 py-1.5 text-right w-24">합계</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-[var(--border-soft)]">
                        {selectedDoc.lineItems.map((li) => (
                          <tr key={li.id} className="hover:bg-gray-50/50">
                            <td className="px-2.5 py-1.5 font-medium text-[var(--ink)]">{li.name || li.item}</td>
                            <td className="px-2 py-1.5 text-right font-mono">{li.qty}</td>
                            <td className="px-2 py-1.5 text-right font-mono text-[var(--steel)]">
                              {li.unitPrice.toLocaleString()}원
                            </td>
                            <td className="px-2.5 py-1.5 text-right font-mono font-bold text-[var(--ink)]">
                              {(li.totalPrice || li.amount || 0).toLocaleString()}원
                            </td>
                          </tr>
                        ))}
                      </tbody>
                      <tfoot className="bg-[var(--canvas)] border-t border-[var(--border)] font-bold text-[11px]">
                        <tr>
                          <td colSpan={3} className="px-2.5 py-1.5 text-right text-[var(--steel)]">
                            총 합계 금액:
                          </td>
                          <td className="px-2.5 py-1.5 text-right font-mono text-blue-900">
                            {selectedDoc.lineItems
                              .reduce((s, x) => s + (x.totalPrice || x.amount || 0), 0)
                              .toLocaleString()}
                            원
                          </td>
                        </tr>
                      </tfoot>
                    </table>
                  </div>
                </div>
              )}

              {/* 다단계 결재선 (Multi-stage Approval Pipeline) */}
              <div className="space-y-2">
                <div className="text-[11px] font-semibold text-[var(--steel)]">결재선 진행 현황</div>
                <div className="space-y-2">
                  {selectedDoc.stages.map((st, idx) => (
                    <div
                      key={idx}
                      className={`p-2 rounded border text-xs flex items-center justify-between ${
                        st.status === "approved"
                          ? "border-emerald-200 bg-emerald-50/60 text-emerald-950"
                          : st.status === "rejected"
                          ? "border-red-200 bg-red-50/60 text-red-950"
                          : "border-[var(--border)] bg-[var(--surface)] text-[var(--steel)]"
                      }`}
                    >
                      <div className="flex items-center gap-2">
                        <span className="w-5 h-5 rounded-full bg-gray-200 text-gray-800 text-[10px] font-bold flex items-center justify-center">
                          {st.step}
                        </span>
                        <div>
                          <div className="font-semibold text-[11px] text-[var(--ink)]">
                            {st.approverRole} · {st.approverName}
                          </div>
                          {st.actingCapacityName && (
                            <div className="flex items-center gap-1.5 mt-0.5">
                              <span className="text-[10px] font-medium px-1.5 py-0.2 rounded bg-indigo-50 dark:bg-indigo-950/60 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-800">
                                서명 자격: [{st.actingCapacityName}]
                              </span>
                              {st.passkeyVerified && (
                                <span className="inline-flex items-center gap-0.5 text-[10px] text-emerald-600 dark:text-emerald-400 font-bold">
                                  <Fingerprint size={10} />
                                  Passkey 생체서명
                                </span>
                              )}
                            </div>
                          )}
                          {st.comment && <div className="text-[10px] text-gray-600 mt-0.5">"{st.comment}"</div>}
                        </div>
                      </div>

                      <div className="text-right">
                        <StatusChip
                          label={st.status === "approved" ? "승인" : st.status === "rejected" ? "반려" : "대기"}
                          tone={st.status === "approved" ? "ok" : st.status === "rejected" ? "danger" : "warn"}
                          size="xs"
                        />
                        {st.timestamp && <div className="text-[9px] font-mono text-gray-500 mt-0.5">{st.timestamp}</div>}
                      </div>
                    </div>
                  ))}
                </div>
              </div>

              {/* 연결된 개체 칩 (Palantir Object Graph) */}
              {selectedDoc.linkedObjects && selectedDoc.linkedObjects.length > 0 && (
                <div className="space-y-1.5 pt-1">
                  <div className="text-[11px] font-semibold text-[var(--steel)]">참조 연결 개체</div>
                  <div className="flex flex-wrap gap-1.5">
                    {selectedDoc.linkedObjects.map((code) => (
                      <ObjectLink key={code} code={code} />
                    ))}
                  </div>
                </div>
              )}

              {/* 액션 컨트롤 바 */}
              <div className="pt-3 border-t border-[var(--border)] flex items-center justify-between gap-2">
                {selectedDoc.status === "초안" ? (
                  <Button
                    size="sm"
                    variant="brand"
                    className="w-full"
                    leftIcon={<Send className="w-3.5 h-3.5" />}
                    onClick={() => submitApproval(selectedDoc.id)}
                  >
                    결재선 상신 제출
                  </Button>
                ) : selectedDoc.status === "결재대기" ? (
                  <>
                    <Button
                      size="sm"
                      variant="danger"
                      className="flex-1"
                      leftIcon={<XCircle className="w-3.5 h-3.5" />}
                      onClick={() => rejectDoc(selectedDoc.id, `${viewAsRole} (나)`, "규정 미달 반려")}
                    >
                      반려
                    </Button>
                    <Button
                      size="sm"
                      variant="brand"
                      className="flex-1"
                      leftIcon={<CheckCircle2 className="w-3.5 h-3.5" />}
                      onClick={() => handleTriggerApproval(selectedDoc)}
                    >
                      결재 승인
                    </Button>
                  </>
                ) : selectedDoc.status === "승인완료" ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    className="w-full"
                    leftIcon={<FileCheck className="w-3.5 h-3.5" />}
                    onClick={() => closeDoc(selectedDoc.id)}
                  >
                    기안자 최종 확인 및 문서 종결
                  </Button>
                ) : (
                  <div className="w-full text-center text-xs text-[var(--faint)] py-1 font-mono">
                    문서 상태: {selectedDoc.status}
                  </div>
                )}
              </div>
            </div>
          ) : (
            <div className="h-64 rounded-lg border border-[var(--border)] bg-[var(--surface)] flex items-center justify-center text-xs text-[var(--faint)]">
              문서를 선택하십시오.
            </div>
          )}
        </div>
      </div>

      {/* 3. 기안문서 작성 모달 (SAP Document Parking / Direct Submission) */}
      {composerOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-xs p-4">
          <div className="w-full max-w-xl rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-5 space-y-4 animate-in fade-in zoom-in-95 duration-150 max-h-[90vh] overflow-y-auto">
            <div className="flex items-center justify-between border-b border-[var(--border)] pb-3">
              <div className="flex items-center gap-2">
                <FileText className="w-5 h-5 text-blue-600" />
                <h3 className="text-sm font-bold text-[var(--ink)]">신규 전자결재 기안 작성 (품의 및 전표)</h3>
              </div>
              <button
                onClick={() => setComposerOpen(false)}
                className="text-gray-400 hover:text-gray-600 cursor-pointer"
              >
                ×
              </button>
            </div>

            <div className="space-y-3 text-xs">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="font-semibold block mb-1">문서 분류</label>
                  <select
                    value={formCategory}
                    onChange={(e) => setFormCategory(e.target.value as any)}
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                  >
                    <option value="연장근로">연장근로</option>
                    <option value="지출결의">지출결의</option>
                    <option value="계약승인">계약승인</option>
                    <option value="규정개정">규정개정</option>
                    <option value="휴가">휴가</option>
                    <option value="기타">기타</option>
                  </select>
                </div>
                <div>
                  <label className="font-semibold block mb-1">연계 개체 코드 (Palantir Graph)</label>
                  <input
                    type="text"
                    value={formLinkedCode}
                    onChange={(e) => setFormLinkedCode(e.target.value)}
                    placeholder="예: C-207, WO-2641, AT-0703-01"
                    className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)] font-mono"
                  />
                </div>
              </div>

              <div>
                <label className="font-semibold block mb-1">기안 제목 *</label>
                <input
                  type="text"
                  required
                  value={formTitle}
                  onChange={(e) => setFormTitle(e.target.value)}
                  placeholder="예: 7월 인천물류 1라인 연장근로 사전 승인의 건"
                  className="w-full h-8 px-2.5 rounded border border-[var(--border)] bg-[var(--surface)]"
                />
              </div>

              <div>
                <label className="font-semibold block mb-1">상세 품의 사유</label>
                <textarea
                  rows={3}
                  value={formContent}
                  onChange={(e) => setFormContent(e.target.value)}
                  placeholder="품의 사유, 대상 인원, 일정 및 세부 내역을 입력하십시오."
                  className="w-full p-2.5 rounded border border-[var(--border)] bg-[var(--surface)] leading-relaxed"
                />
              </div>

              {/* 품목별 청구 항목 편집기 */}
              <div className="space-y-2 pt-2 border-t border-[var(--border)]">
                <div className="flex items-center justify-between">
                  <span className="font-semibold flex items-center gap-1">
                    <Receipt className="w-3.5 h-3.5 text-blue-600" />
                    <span>품목별 지출·청구 내역 (선택)</span>
                  </span>
                  <button
                    type="button"
                    onClick={addLineItem}
                    className="text-xs text-[var(--brand)] hover:underline flex items-center gap-0.5 cursor-pointer"
                  >
                    <Plus className="w-3 h-3" />
                    <span>품목 추가</span>
                  </button>
                </div>

                <div className="space-y-2">
                  {formLineItems.map((li, idx) => (
                    <div key={idx} className="flex items-center gap-2">
                      <input
                        type="text"
                        placeholder="품목 / 규격명"
                        value={li.name}
                        onChange={(e) => updateLineItem(idx, "name", e.target.value)}
                        className="flex-1 h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)]"
                      />
                      <input
                        type="number"
                        placeholder="수량"
                        value={li.qty}
                        onChange={(e) => updateLineItem(idx, "qty", e.target.value)}
                        className="w-14 h-7 px-1.5 rounded border border-[var(--border)] bg-[var(--surface)] text-right font-mono"
                      />
                      <input
                        type="number"
                        placeholder="단가"
                        value={li.unitPrice}
                        onChange={(e) => updateLineItem(idx, "unitPrice", e.target.value)}
                        className="w-24 h-7 px-2 rounded border border-[var(--border)] bg-[var(--surface)] text-right font-mono"
                      />
                      <div className="w-24 text-right font-mono font-bold text-[var(--ink)]">
                        {li.totalPrice.toLocaleString()}원
                      </div>
                      <button
                        type="button"
                        onClick={() => removeLineItem(idx)}
                        className="text-gray-400 hover:text-red-600 cursor-pointer p-1"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    </div>
                  ))}
                </div>

                <div className="flex items-center justify-between p-2 rounded bg-blue-50 border border-blue-200 text-xs">
                  <span className="text-blue-900 font-semibold">자동 산정 DoA 합계 금액:</span>
                  <span className="font-mono font-bold text-sm text-blue-950">
                    {computedTotalAmount.toLocaleString()}원
                  </span>
                </div>
              </div>
            </div>

            <div className="flex items-center justify-between pt-3 border-t border-[var(--border)]">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => handleCreateDraft(false)}
                title="입력≠적용 원칙: 검증 없이 임시 저장"
              >
                예비 저장 (파킹)
              </Button>

              <div className="flex items-center gap-2">
                <Button variant="ghost" size="sm" onClick={() => setComposerOpen(false)}>
                  취소
                </Button>
                <Button variant="brand" size="sm" onClick={() => handleCreateDraft(true)}>
                  상신 제출 (게이트 검증)
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* 4. Passkey FIDO2 WebAuthn 생체인증 모달 */}
      {passkeyModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4">
          <div className="w-full max-w-sm rounded-xl border border-[var(--border)] bg-[var(--surface)] shadow-2xl p-6 text-center space-y-4 animate-in fade-in zoom-in-95 duration-200">
            <div className="w-12 h-12 rounded-full bg-emerald-100 text-emerald-700 flex items-center justify-center mx-auto">
              <KeyRound className="w-6 h-6 animate-pulse" />
            </div>

            <div className="space-y-1">
              <h3 className="text-sm font-bold text-[var(--ink)]">FIDO2 Passkey 법적 서명</h3>
              <p className="text-xs text-[var(--steel)]">
                본 결재({passkeyTargetDoc?.code})는 중요 법적·재무적 승인 건입니다.
              </p>
            </div>

            <div className="p-3 rounded-lg border border-emerald-200 bg-emerald-50 text-[11px] text-emerald-950 font-mono text-left space-y-1">
              <div>RP ID: console.oyatie.com</div>
              <div>타겟: {passkeyTargetDoc?.code} ({passkeyTargetDoc?.title})</div>
              <div>서명자: {viewAsRole} (박지영)</div>
            </div>

            {passkeyCeremonyStep === "challenge" && (
              <Button
                variant="brand"
                size="md"
                className="w-full font-bold"
                onClick={executePasskeyVerification}
              >
                생체인증 (지문 / Touch ID) 요청
              </Button>
            )}

            {passkeyCeremonyStep === "touch" && (
              <div className="py-2 text-xs font-semibold text-emerald-700 animate-pulse">
                보안 키 또는 지문 센서를 터치하십시오...
              </div>
            )}

            {passkeyCeremonyStep === "verified" && (
              <div className="py-2 text-xs font-bold text-emerald-700 flex items-center justify-center gap-1">
                <CheckCircle2 className="w-4 h-4 text-emerald-600" />
                <span>서명 및 수령 증빙 검증 완료!</span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* 직무 전결 자격 기반 전자서명 모달 (전자서명법 제3조) */}
      {capacityTargetDoc && (
        <CapacitySignaturePicker
          doc={capacityTargetDoc}
          approverId={activeApprover.id}
          approverName={activeApprover.name}
          isOpen={capacitySignModalOpen}
          onClose={() => setCapacitySignModalOpen(false)}
          onSign={handleCapacitySign}
        />
      )}
    </div>
  );
}
