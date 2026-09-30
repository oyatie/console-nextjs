// ErrorBoundary.tsx — Enterprise Resilient React Error Boundary
"use client";

import React, { Component, ErrorInfo, ReactNode } from "react";
import { AlertOctagon, RotateCcw, Home } from "lucide-react";
import { Button } from "./Button";

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  errorInfo: ErrorInfo | null;
}

export class ErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false,
    error: null,
    errorInfo: null,
  };

  public static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error, errorInfo: null };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("Uncaught enterprise component error:", error, errorInfo);
    this.setState({ errorInfo });
  }

  private handleReset = () => {
    this.setState({ hasError: false, error: null, errorInfo: null });
    if (typeof window !== "undefined") {
      window.location.reload();
    }
  };

  private handleGoHome = () => {
    if (typeof window !== "undefined") {
      window.location.href = "/dashboard";
    }
  };

  public render() {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback;
      }

      return (
        <div className="min-h-screen w-full flex items-center justify-center bg-[var(--canvas)] p-6 text-[var(--foreground)]">
          <div className="max-w-lg w-full p-6 rounded-2xl border border-[var(--border)] bg-[var(--surface)] shadow-lg space-y-4">
            <div className="flex items-center gap-3 text-red-600">
              <span className="p-2 rounded-xl bg-red-50 dark:bg-red-950/40 border border-red-200 dark:border-red-900">
                <AlertOctagon className="w-6 h-6" />
              </span>
              <div>
                <h2 className="text-base font-bold">런타임 오류 감지 및 세션 보호</h2>
                <p className="text-xs text-[var(--muted)]">화면 렌더링 중 예기치 않은 예외가 발생하였습니다.</p>
              </div>
            </div>

            <div className="p-3 rounded-lg border border-[var(--border)] bg-[var(--background)] font-mono text-[11px] text-red-600 overflow-x-auto max-h-36">
              {this.state.error?.message || "알 수 없는 컴포넌트 렌더링 예외"}
            </div>

            <div className="text-xs text-[var(--muted)] leading-relaxed">
              이 화면은 시연용입니다. 변경 내용은 현재 탭의 메모리에만 남으며 새로고침하면 사라집니다.
            </div>

            <div className="flex items-center gap-3 pt-2 border-t border-[var(--border)]">
              <Button size="sm" variant="primary" onClick={this.handleReset} className="flex-1">
                <RotateCcw className="w-3.5 h-3.5 mr-1.5" /> 화면 새로고침
              </Button>
              <Button size="sm" variant="outline" onClick={this.handleGoHome} className="flex-1">
                <Home className="w-3.5 h-3.5 mr-1.5" /> 대시보드로 이동
              </Button>
            </div>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
