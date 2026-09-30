"use client";

import React, { useState } from "react";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { StatusChip } from "@/components/ui/StatusChip";
import { Button } from "@/components/ui/Button";
import {
  Mail,
  Inbox,
  Send,
  Archive,
  Trash2,
  Reply,
  ShieldCheck,
  PlusCircle,
  Search,
  CheckCircle2,
  FileCheck2,
  Clock,
  User,
  Filter,
} from "lucide-react";
import { useAppStore } from "@/lib/store";
import { EmailItem } from "@/lib/types";
import { MailComposerModal } from "@/components/comms/MailComposerModal";

export default function MailPage() {
  const emails = useAppStore((s) => s.emails);
  const markEmailAsRead = useAppStore((s) => s.markEmailAsRead);
  const archiveEmail = useAppStore((s) => s.archiveEmail);
  const deleteEmail = useAppStore((s) => s.deleteEmail);
  const createApprovalDraftFromEmail = useAppStore((s) => s.createApprovalDraftFromEmail);

  const [activeFolder, setActiveFolder] = useState<"inbox" | "sent" | "archive" | "trash">("inbox");
  const [selectedMailId, setSelectedMailId] = useState<string>(emails[0]?.id || "");
  const [searchQuery, setSearchQuery] = useState("");

  // 모달 상태
  const [composerOpen, setComposerOpen] = useState(false);
  const [replyConfig, setReplyConfig] = useState<{
    recipient: string;
    recipientEmail: string;
    subject: string;
    body: string;
    linkedObject?: string;
  } | null>(null);

  // 현재 폴더에 따른 메일 목록
  const folderEmails = emails.filter((m) => m.folder === activeFolder);
  const filteredEmails = folderEmails.filter((m) => {
    if (!searchQuery.trim()) return true;
    const q = searchQuery.toLowerCase();
    return (
      m.subject.toLowerCase().includes(q) ||
      m.sender.toLowerCase().includes(q) ||
      m.preview.toLowerCase().includes(q) ||
      (m.linkedObject && m.linkedObject.toLowerCase().includes(q))
    );
  });

  const selectedMail = emails.find((m) => m.id === selectedMailId) || filteredEmails[0] || null;

  // 카운트 계산
  const inboxCount = emails.filter((m) => m.folder === "inbox").length;
  const inboxUnread = emails.filter((m) => m.folder === "inbox" && m.isUnread).length;
  const sentCount = emails.filter((m) => m.folder === "sent").length;
  const archiveCount = emails.filter((m) => m.folder === "archive").length;
  const trashCount = emails.filter((m) => m.folder === "trash").length;

  const handleSelectMail = (mail: EmailItem) => {
    setSelectedMailId(mail.id);
    if (mail.isUnread) {
      markEmailAsRead(mail.id);
    }
  };

  const handleOpenCompose = () => {
    setReplyConfig(null);
    setComposerOpen(true);
  };

  const handleReply = (mail: EmailItem) => {
    setReplyConfig({
      recipient: mail.sender,
      recipientEmail: mail.senderEmail,
      subject: mail.subject.startsWith("Re:") ? mail.subject : `Re: ${mail.subject}`,
      body: `\n\n--- 원문 메일 (${mail.sender} / ${mail.date}) ---\n${mail.body}`,
      linkedObject: mail.linkedObject,
    });
    setComposerOpen(true);
  };

  const handleConvertToApproval = (mail: EmailItem) => {
    createApprovalDraftFromEmail(mail.id);
  };

  return (
    <div className="h-[calc(100vh-8.5rem)] rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs overflow-hidden flex text-xs">
      {/* 1열: 메일 폴더 네비게이션 */}
      <div className="w-52 border-r border-[var(--border)] bg-gray-50/50 p-2.5 space-y-3 shrink-0 select-none flex flex-col justify-between">
        <div className="space-y-1">
          <Button
            size="sm"
            variant="brand"
            className="w-full justify-center mb-3 font-semibold"
            leftIcon={<PlusCircle size={14} />}
            onClick={handleOpenCompose}
          >
            새 메일 작성
          </Button>

          <div className="px-2 py-1 text-[10px] font-bold text-[var(--faint)] uppercase tracking-wider">
            사내 사서함 (Mailboxes)
          </div>

          <button
            onClick={() => setActiveFolder("inbox")}
            className={`w-full flex items-center justify-between px-2.5 py-2 rounded-md cursor-pointer transition-colors ${
              activeFolder === "inbox"
                ? "bg-amber-100 text-amber-950 font-bold"
                : "text-[var(--steel)] hover:bg-gray-200/50 hover:text-[var(--ink)]"
            }`}
          >
            <div className="flex items-center gap-2">
              <Inbox className="w-4 h-4" />
              <span>받은 편지함</span>
            </div>
            {inboxUnread > 0 ? (
              <span className="font-mono text-[10px] bg-amber-500 text-white font-bold px-1.5 py-0.2 rounded-full">
                {inboxUnread}
              </span>
            ) : (
              <span className="font-mono text-[10px] text-[var(--faint)]">{inboxCount}</span>
            )}
          </button>

          <button
            onClick={() => setActiveFolder("sent")}
            className={`w-full flex items-center justify-between px-2.5 py-2 rounded-md cursor-pointer transition-colors ${
              activeFolder === "sent"
                ? "bg-amber-100 text-amber-950 font-bold"
                : "text-[var(--steel)] hover:bg-gray-200/50 hover:text-[var(--ink)]"
            }`}
          >
            <div className="flex items-center gap-2">
              <Send className="w-4 h-4" />
              <span>보낸 편지함</span>
            </div>
            <span className="font-mono text-[10px] text-[var(--faint)]">{sentCount}</span>
          </button>

          <button
            onClick={() => setActiveFolder("archive")}
            className={`w-full flex items-center justify-between px-2.5 py-2 rounded-md cursor-pointer transition-colors ${
              activeFolder === "archive"
                ? "bg-amber-100 text-amber-950 font-bold"
                : "text-[var(--steel)] hover:bg-gray-200/50 hover:text-[var(--ink)]"
            }`}
          >
            <div className="flex items-center gap-2">
              <Archive className="w-4 h-4" />
              <span>보관함</span>
            </div>
            <span className="font-mono text-[10px] text-[var(--faint)]">{archiveCount}</span>
          </button>

          <button
            onClick={() => setActiveFolder("trash")}
            className={`w-full flex items-center justify-between px-2.5 py-2 rounded-md cursor-pointer transition-colors ${
              activeFolder === "trash"
                ? "bg-amber-100 text-amber-950 font-bold"
                : "text-[var(--steel)] hover:bg-gray-200/50 hover:text-[var(--ink)]"
            }`}
          >
            <div className="flex items-center gap-2">
              <Trash2 className="w-4 h-4" />
              <span>휴지통</span>
            </div>
            <span className="font-mono text-[10px] text-[var(--faint)]">{trashCount}</span>
          </button>
        </div>

        {/* 하단 시스템 인증 현황 배너 */}
        <div className="p-2.5 rounded-lg border border-[var(--border)] bg-white/70 space-y-1 text-[10px] text-[var(--steel)]">
          <div className="flex items-center gap-1 font-semibold text-emerald-800">
            <ShieldCheck size={12} className="text-emerald-600" />
            <span>도메인 암호화 연동</span>
          </div>
          <p className="leading-tight text-[var(--faint)]">
            전자금융감독규정 및 개인정보보호법에 의거 TLS 1.3 암호 통신이 적용됩니다.
          </p>
        </div>
      </div>

      {/* 2열: 메일 목록 */}
      <div className="w-80 border-r border-[var(--border)] flex flex-col bg-[var(--surface)] shrink-0">
        {/* 상단 검색 바 */}
        <div className="p-2.5 border-b border-[var(--border)] bg-gray-50/40 space-y-2">
          <div className="flex items-center justify-between">
            <span className="font-bold text-[var(--ink)] capitalize">
              {activeFolder === "inbox" && "받은 편지함"}
              {activeFolder === "sent" && "보낸 편지함"}
              {activeFolder === "archive" && "보관함"}
              {activeFolder === "trash" && "휴지통"}
            </span>
            <span className="text-[11px] text-[var(--faint)]">{filteredEmails.length}개 항목</span>
          </div>
          <div className="relative">
            <Search className="w-3.5 h-3.5 text-[var(--faint)] absolute left-2.5 top-2" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="제목, 발신자, 개체 코드 검색..."
              className="w-full pl-8 pr-2.5 py-1 text-xs rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] placeholder:text-[var(--faint)] focus:outline-none"
            />
          </div>
        </div>

        {/* 메일 리스트 */}
        <div className="flex-1 overflow-y-auto divide-y divide-[var(--border-soft)]">
          {filteredEmails.length === 0 ? (
            <div className="p-8 text-center text-[var(--faint)]">
              {searchQuery ? "검색 결과가 없습니다." : "메일이 없습니다."}
            </div>
          ) : (
            filteredEmails.map((m) => (
              <div
                key={m.id}
                onClick={() => handleSelectMail(m)}
                className={`p-3 cursor-pointer transition-colors space-y-1.5 ${
                  selectedMail?.id === m.id
                    ? "bg-amber-50/80 border-l-2 border-l-[var(--signal)]"
                    : "hover:bg-gray-50"
                }`}
              >
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-1.5 min-w-0">
                    {m.isUnread && (
                      <span className="w-1.5 h-1.5 rounded-full bg-amber-500 shrink-0" />
                    )}
                    <span
                      className={`truncate ${
                        m.isUnread ? "text-[var(--ink)] font-bold" : "text-[var(--steel)]"
                      }`}
                    >
                      {activeFolder === "sent" ? `수신: ${m.recipient}` : m.sender}
                    </span>
                  </div>
                  <span className="font-mono text-[10px] text-[var(--faint)] shrink-0 ml-1">
                    {m.date}
                  </span>
                </div>
                <div
                  className={`line-clamp-1 ${
                    m.isUnread ? "font-bold text-[var(--ink)]" : "font-medium text-[var(--ink)]"
                  }`}
                >
                  {m.subject}
                </div>
                <div className="text-[11px] text-[var(--steel)] line-clamp-1">{m.preview}</div>
                {m.linkedObject && (
                  <div className="pt-0.5">
                    <ObjectLink code={m.linkedObject} />
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      </div>

      {/* 3열: 메일 본문 리더 */}
      <div className="flex-1 flex flex-col bg-[var(--surface)] min-w-0">
        {selectedMail ? (
          <>
            {/* 메일 헤더 및 액션 툴바 */}
            <div className="p-4 border-b border-[var(--border)] space-y-3 bg-gray-50/20">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h2 className="text-sm font-bold text-[var(--ink)]">{selectedMail.subject}</h2>
                <div className="flex items-center gap-1.5">
                  <Button
                    size="xs"
                    variant="secondary"
                    leftIcon={<Reply size={12} />}
                    onClick={() => handleReply(selectedMail)}
                  >
                    답장
                  </Button>
                  <Button
                    size="xs"
                    variant="brand"
                    leftIcon={<FileCheck2 size={12} />}
                    onClick={() => handleConvertToApproval(selectedMail)}
                  >
                    결재 기안으로 변환
                  </Button>
                  {selectedMail.folder !== "archive" && (
                    <Button
                      size="xs"
                      variant="ghost"
                      leftIcon={<Archive size={12} />}
                      onClick={() => archiveEmail(selectedMail.id)}
                      title="보관함으로 이동"
                    >
                      보관
                    </Button>
                  )}
                  {selectedMail.folder !== "trash" && (
                    <Button
                      size="xs"
                      variant="ghost"
                      leftIcon={<Trash2 size={12} />}
                      onClick={() => deleteEmail(selectedMail.id)}
                      title="휴지통으로 이동"
                    >
                      삭제
                    </Button>
                  )}
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-between text-[11px] text-[var(--steel)] gap-2">
                <div>
                  <span className="font-bold text-[var(--ink)]">{selectedMail.sender}</span> &lt;
                  {selectedMail.senderEmail}&gt;
                  <span className="mx-2 text-gray-300">→</span>
                  <span>{selectedMail.recipient}</span> &lt;{selectedMail.recipientEmail}&gt;
                </div>
                <span className="font-mono">{selectedMail.date}</span>
              </div>

              {selectedMail.linkedObject && (
                <div className="flex items-center gap-2 pt-2 border-t border-[var(--border-soft)]">
                  <span className="text-[10px] text-[var(--steel)] font-semibold">
                    연계된 업무 개체:
                  </span>
                  <ObjectLink code={selectedMail.linkedObject} />
                  <span className="text-[10px] text-[var(--faint)]">
                    (마우스를 올리면 실시간 요약 카드 및 1-클릭 승인 액션이 활성화됩니다)
                  </span>
                </div>
              )}
            </div>

            {/* 메일 본문 내용 */}
            <div className="flex-1 p-6 overflow-y-auto font-sans leading-relaxed whitespace-pre-wrap text-[var(--ink)] text-xs">
              {selectedMail.body}
            </div>
          </>
        ) : (
          <div className="flex-1 flex flex-col items-center justify-center text-[var(--faint)] space-y-2">
            <Mail size={32} className="opacity-40" />
            <span>열람할 메일을 선택하십시오.</span>
          </div>
        )}
      </div>

      {/* 신규 메일 작성 및 답장 모달 */}
      <MailComposerModal
        isOpen={composerOpen}
        onClose={() => setComposerOpen(false)}
        initialRecipient={replyConfig?.recipient || ""}
        initialRecipientEmail={replyConfig?.recipientEmail || ""}
        initialSubject={replyConfig?.subject || ""}
        initialBody={replyConfig?.body || ""}
        initialLinkedObject={replyConfig?.linkedObject || ""}
      />
    </div>
  );
}
