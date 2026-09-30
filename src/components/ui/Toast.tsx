"use client";

import React, { useEffect } from "react";
import { useAppStore } from "@/lib/store";
import { ToastMessage } from "@/lib/types";
import { CheckCircle2, AlertCircle, Info, AlertTriangle, X } from "lucide-react";

export function ToastContainer() {
  const toasts = useAppStore((s) => s.toasts);
  const removeToast = useAppStore((s) => s.removeToast);

  return (
    <div
      aria-live="polite"
      className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 max-w-sm pointer-events-none"
    >
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} onDismiss={() => removeToast(t.id)} />
      ))}
    </div>
  );
}

function ToastItem({
  toast,
  onDismiss,
}: {
  toast: ToastMessage;
  onDismiss: () => void;
}) {
  useEffect(() => {
    const timer = setTimeout(() => {
      onDismiss();
    }, 4500);
    return () => clearTimeout(timer);
  }, [onDismiss]);

  let Icon = Info;
  let borderTone = "border-blue-300 bg-white text-blue-900";
  let iconColor = "text-blue-600";

  if (toast.tone === "ok") {
    Icon = CheckCircle2;
    borderTone = "border-emerald-300 bg-white text-emerald-950";
    iconColor = "text-emerald-600";
  } else if (toast.tone === "warn") {
    Icon = AlertTriangle;
    borderTone = "border-amber-300 bg-white text-amber-950";
    iconColor = "text-amber-600";
  } else if (toast.tone === "danger") {
    Icon = AlertCircle;
    borderTone = "border-red-300 bg-white text-red-950";
    iconColor = "text-red-600";
  }

  return (
    <div
      className={`pointer-events-auto flex items-start gap-2.5 p-3 rounded-lg border shadow-xl transition-all duration-200 animate-in fade-in slide-in-from-bottom-2 ${borderTone}`}
      role="alert"
    >
      <Icon className={`w-4 h-4 shrink-0 mt-0.5 ${iconColor}`} />
      <div className="flex-1 text-xs">
        <h5 className="font-bold text-[var(--ink)] leading-snug">{toast.title}</h5>
        {toast.description && (
          <p className="text-[11px] text-[var(--steel)] mt-0.5 leading-relaxed">
            {toast.description}
          </p>
        )}
        {toast.actionLabel && toast.onAction && (
          <button
            type="button"
            onClick={() => {
              toast.onAction?.();
              onDismiss();
            }}
            className="mt-1 text-[11px] font-bold text-amber-700 hover:text-amber-900 underline cursor-pointer"
          >
            {toast.actionLabel}
          </button>
        )}
      </div>
      <button
        type="button"
        onClick={onDismiss}
        className="text-[var(--faint)] hover:text-[var(--ink)] p-0.5 rounded cursor-pointer"
        aria-label="닫기"
      >
        <X className="w-3.5 h-3.5" />
      </button>
    </div>
  );
}
