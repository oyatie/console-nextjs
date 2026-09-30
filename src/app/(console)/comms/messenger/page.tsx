"use client";

import React, { useState } from "react";
import { useAppStore } from "@/lib/store";
import { ObjectLink } from "@/components/ui/ObjectLink";
import { Button } from "@/components/ui/Button";
import { DynamicText } from "@/components/ui/DynamicText";
import { EntityMentionPicker } from "@/components/ui/EntityMentionPicker";
import {
  MessageSquare,
  Hash,
  Layers,
  Send,
  Users,
  Search,
  Plus,
} from "lucide-react";
import { CreateThreadModal } from "@/components/comms/CreateThreadModal";

export default function MessengerPage() {
  const threads = useAppStore((s) => s.threads);
  const messages = useAppStore((s) => s.messages);
  const sendMessage = useAppStore((s) => s.sendMessage);
  const createThreadForObject = useAppStore((s) => s.createThreadForObject);

  const [activeThreadId, setActiveThreadId] = useState<string>(threads[0]?.id || "");
  const [inputVal, setInputVal] = useState("");
  const [searchQ, setSearchQ] = useState("");
  const [mentionTrigger, setMentionTrigger] = useState<"#" | "@" | "[" | null>(null);
  const [mentionQuery, setMentionQuery] = useState("");
  const [createThreadOpen, setCreateThreadOpen] = useState(false);

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const val = e.target.value;
    setInputVal(val);
    const match = val.match(/([#@\[])([\w가-힣]*)$/);
    if (match) {
      setMentionTrigger(match[1] as "#" | "@" | "[");
      setMentionQuery(match[2] || "");
    } else {
      setMentionTrigger(null);
      setMentionQuery("");
    }
  };

  const handleMentionSelect = (tokenText: string) => {
    if (!mentionTrigger) return;
    const lastIndex = inputVal.lastIndexOf(mentionTrigger);
    if (lastIndex !== -1) {
      const updated = inputVal.substring(0, lastIndex) + tokenText + " ";
      setInputVal(updated);
    }
    setMentionTrigger(null);
    setMentionQuery("");
  };

  const activeThread = threads.find((t) => t.id === activeThreadId) || threads[0];
  const activeMessages = activeThread ? messages[activeThread.id] || [] : [];

  const handleSend = (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputVal.trim() || !activeThread) return;
    sendMessage(activeThread.id, inputVal.trim());
    setInputVal("");
  };

  return (
    <div className="h-[calc(100vh-8.5rem)] rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-2xs overflow-hidden flex">
      {/* 좌측 채널 및 스레드 목록 (Slack Channel List) */}
      <div className="w-64 border-r border-[var(--border)] flex flex-col bg-gray-50/50 shrink-0 select-none">
        {/* 채널 검색 및 신규 생성 */}
        <div className="p-2.5 border-b border-[var(--border)] space-y-2">
          <Button
            size="xs"
            variant="brand"
            className="w-full justify-center font-semibold"
            leftIcon={<Plus size={12} />}
            onClick={() => setCreateThreadOpen(true)}
          >
            새 채널 / 스레드 개설
          </Button>

          <div className="relative">
            <Search className="w-3 h-3 text-[var(--faint)] absolute left-2.5 top-2" />
            <input
              type="text"
              value={searchQ}
              onChange={(e) => setSearchQ(e.target.value)}
              placeholder="채널, 스레드 검색..."
              className="w-full pl-7 pr-2.5 py-1 text-xs rounded border border-[var(--border)] bg-[var(--surface)] text-[var(--ink)] placeholder:text-[var(--faint)] focus:outline-none"
            />
          </div>
        </div>

        {/* 채널 및 객체 스레드 목록 */}
        <div className="flex-1 overflow-y-auto p-2 space-y-4 text-xs">
          {/* 객체 맥락 스레드 */}
          <div className="space-y-1">
            <div className="px-2 text-[10px] font-bold text-[var(--steel)] tracking-wider uppercase">
              개체 맥락 스레드 (Context Threads)
            </div>
            {threads
              .filter((t) => t.objectRef)
              .map((t) => (
                <div
                  key={t.id}
                  onClick={() => setActiveThreadId(t.id)}
                  className={`px-2.5 py-2 rounded-md cursor-pointer transition-colors ${
                    activeThread?.id === t.id
                      ? "bg-amber-100 text-amber-950 font-bold"
                      : "text-[var(--ink)] hover:bg-gray-200/50"
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-blue-700 text-[11px]">{t.objectRef}</span>
                    <span className="text-[10px] text-[var(--faint)]">{t.updatedAt}</span>
                  </div>
                  <div className="text-[11px] truncate mt-0.5">{t.title}</div>
                </div>
              ))}
          </div>

          {/* 공용 채널 */}
          <div className="space-y-1">
            <div className="px-2 text-[10px] font-bold text-[var(--steel)] tracking-wider uppercase">
              조직 채널 (Channels)
            </div>
            {threads
              .filter((t) => !t.objectRef)
              .map((t) => (
                <div
                  key={t.id}
                  onClick={() => setActiveThreadId(t.id)}
                  className={`flex items-center gap-2 px-2.5 py-1.5 rounded-md cursor-pointer transition-colors ${
                    activeThread?.id === t.id
                      ? "bg-amber-100 text-amber-950 font-bold"
                      : "text-[var(--steel)] hover:bg-gray-200/50 hover:text-[var(--ink)]"
                  }`}
                >
                  <Hash className="w-3.5 h-3.5 opacity-60" />
                  <span className="truncate">{t.title}</span>
                </div>
              ))}
          </div>
        </div>
      </div>

      {/* 우측 활성 대화창 (Active Chat View) */}
      <div className="flex-1 flex flex-col bg-[var(--surface)] min-w-0">
        {activeThread ? (
          <>
            {/* 스레드 상단 정보 헤더 */}
            <div className="px-4 py-2.5 border-b border-[var(--border)] flex items-center justify-between bg-gray-50/30">
              <div>
                <div className="flex items-center gap-2">
                  {activeThread.objectRef ? (
                    <ObjectLink code={activeThread.objectRef} />
                  ) : (
                    <Hash className="w-4 h-4 text-[var(--steel)]" />
                  )}
                  <h3 className="text-xs font-bold text-[var(--ink)]">{activeThread.title}</h3>
                </div>
                <div className="text-[10px] text-[var(--steel)] mt-0.5">
                  참여자: {activeThread.participantNames.join(", ")}
                </div>
              </div>
            </div>

            {/* 메시지 피드 */}
            <div className="flex-1 overflow-y-auto p-4 space-y-4">
              {activeMessages.length === 0 ? (
                <div className="text-center py-16 text-xs text-[var(--faint)]">
                  이 스레드에 등록된 메시지가 없습니다. 첫 대화를 시작해보세요.
                </div>
              ) : (
                activeMessages.map((m) => (
                  <div key={m.id} className="flex items-start gap-3 group">
                    <div className="w-7 h-7 rounded bg-amber-500/20 text-amber-900 font-bold flex items-center justify-center text-xs shrink-0">
                      {m.avatarText}
                    </div>
                    <div className="flex-1 space-y-1">
                      <div className="flex items-center gap-2">
                        <span className="text-xs font-bold text-[var(--ink)]">{m.authorName}</span>
                        <span className="font-mono text-[10px] text-[var(--faint)]">{m.timestamp}</span>
                      </div>
                      <div className="text-xs text-[var(--ink)] leading-relaxed">
                        <DynamicText text={m.text} />
                      </div>
                      {m.linkedCodes && (
                        <div className="flex items-center gap-1.5 mt-1">
                          {m.linkedCodes.map((code) => (
                            <ObjectLink key={code} code={code} />
                          ))}
                        </div>
                      )}
                    </div>
                  </div>
                ))
              )}
            </div>

            {/* 하단 입력 폼 */}
            <form onSubmit={handleSend} className="relative p-3 border-t border-[var(--border)] bg-[var(--surface)]">
              {mentionTrigger && (
                <EntityMentionPicker
                  query={mentionQuery}
                  triggerType={mentionTrigger}
                  onSelect={handleMentionSelect}
                  onClose={() => setMentionTrigger(null)}
                />
              )}
              <div className="flex items-center gap-2 border border-[var(--border)] rounded-md px-3 py-2 bg-[var(--canvas)] focus-within:border-[var(--steel)] shadow-2xs">
                <input
                  type="text"
                  value={inputVal}
                  onChange={handleInputChange}
                  placeholder="메시지 입력... (@멘션, #개체코드, !커맨드)"
                  className="flex-1 text-xs bg-transparent focus:outline-none text-[var(--ink)] placeholder:text-[var(--faint)]"
                />
                <button
                  type="submit"
                  disabled={!inputVal.trim()}
                  className="p-1 rounded text-amber-600 hover:text-amber-700 disabled:opacity-30 cursor-pointer"
                >
                  <Send className="w-4 h-4" />
                </button>
              </div>
            </form>
          </>
        ) : (
          <div className="flex-1 flex items-center justify-center text-xs text-[var(--faint)]">
            스레드를 선택하십시오.
          </div>
        )}
      </div>

      <CreateThreadModal
        isOpen={createThreadOpen}
        onClose={() => setCreateThreadOpen(false)}
        onCreated={(id) => setActiveThreadId(id)}
      />
    </div>
  );
}
