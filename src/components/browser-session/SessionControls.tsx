"use client";

import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";

async function mutation(path: string, signal: AbortSignal, body?: unknown, csrf?: string) {
  return fetch(path, { method: "POST", credentials: "same-origin", cache: "no-store", signal,
    headers: { ...(body === undefined ? {} : { "Content-Type": "application/json" }), ...(csrf ? { "x-csrf-token": csrf } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body) });
}
export function LoginControl() {
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState("");
  const attempt = useRef<AbortController | null>(null);
  const finishSent = useRef(false);
  useEffect(() => {
    const stop = () => { attempt.current?.abort(); attempt.current = null; };
    window.addEventListener("pagehide", stop);
    return () => { stop(); window.removeEventListener("pagehide", stop); };
  }, []);
  function cancel() {
    attempt.current?.abort(); attempt.current = null;
    setPending(false); setNotice(finishSent.current ? "로그인 요청 이후 대기를 취소했습니다. 결과를 확인할 수 없습니다. 같은 인증을 다시 보내지 말고 새 인증을 시작하세요." : "로그인 인증을 취소했습니다. 다시 시작할 수 있습니다.");
  }
  async function login() {
    if (attempt.current) return;
    if (!window.PublicKeyCredential || !PublicKeyCredential.parseRequestOptionsFromJSON ||
        !PublicKeyCredential.prototype.toJSON || !navigator.credentials) {
      setNotice("이 브라우저에서는 패스키 인증을 사용할 수 없습니다. 지원하는 최신 브라우저에서 다시 로그인하세요."); return;
    }
    const controller = new AbortController(); attempt.current = controller; finishSent.current = false;
    const deadline = setTimeout(() => controller.abort(), 60000);
    let submitted = false;
    setPending(true); setNotice("패스키 인증을 준비하고 있습니다.");
    const active = () => attempt.current === controller && !controller.signal.aborted;
    try {
      const response = await mutation("/api/browser-session/start/", controller.signal);
      if (!active()) return;
      if (!response.ok) { setNotice(response.status === 429 ? "기존 인증을 완료하거나 해당 화면에서 로그아웃하세요. 원래 만료 기한까지 기다린 뒤 다시 시작할 수 있습니다." : "인증을 시작할 수 없습니다. 연결과 인증 설정을 확인한 뒤 다시 시도하세요."); return; }
      const start = await response.json();
      if (!active()) return;
      setNotice("패스키로 본인임을 확인하세요. 인증을 취소할 수 있습니다.");
      const credential = await navigator.credentials.get({ ...start.challenge,
        publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(start.challenge.publicKey), signal: controller.signal });
      if (!active()) return;
      if (!(credential instanceof PublicKeyCredential)) throw new Error();
      submitted = true; finishSent.current = true; setNotice("로그인 결과를 확인하고 있습니다.");
      const finished = await mutation("/api/browser-session/login/", controller.signal,
        { ceremony_id: start.ceremony_id, credential: credential.toJSON() }, start.csrf_token);
      if (!active()) return;
      if (!finished.ok) { setNotice(finished.status >= 500 ? "로그인 결과를 확인할 수 없습니다. 같은 인증을 다시 보내지 말고 새 인증을 시작하세요." : "인증 요청이 거절되었거나 만료되었습니다. 새 인증을 시작하세요."); return; }
      const result = await finished.json();
      if (!active()) return;
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result.context_id) || result.context_id !== start.ceremony_id) throw new Error();
      window.location.assign(`/me/${result.context_id}/attendance/`);
    } catch {
      if (attempt.current === controller) setNotice(submitted ? "로그인 결과를 확인할 수 없습니다. 같은 인증을 다시 보내지 말고 새 인증을 시작하세요." : "인증이 취소되었거나 연결을 확인할 수 없습니다. 새 인증을 시작하세요.");
    } finally {
      clearTimeout(deadline);
      if (attempt.current === controller) { attempt.current = null; setPending(false); }
    }
  }
  return <div className="browser-controls">
    <Button size="lg" variant="primary" className="browser-button" loading={pending} onClick={login}>패스키로 로그인</Button>
    {pending && <Button size="lg" className="browser-button" onClick={cancel}>로그인 인증 취소</Button>}
    <p role={pending || !notice ? "status" : "alert"} aria-live="polite" className="browser-notice">{notice}</p>
  </div>;
}
export function LogoutControl({ context, csrf }: { context: string; csrf: string }) {
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState("");
  const attempt = useRef<AbortController | null>(null);
  useEffect(() => () => { attempt.current?.abort(); attempt.current = null; }, []);
  async function logout() {
    if (attempt.current) return;
    const controller = new AbortController(); attempt.current = controller;
    const deadline = setTimeout(() => controller.abort(), 20000);
    setPending(true); setNotice("이 화면의 로그아웃 결과를 확인하고 있습니다.");
    try {
      const response = await mutation("/api/browser-session/logout/", controller.signal, { browser_context: context }, csrf);
      if (attempt.current !== controller || controller.signal.aborted) return;
      if (response.status !== 204) throw new Error();
      window.location.assign("/login/");
    } catch {
      if (attempt.current === controller) setNotice("로그아웃 결과를 확인할 수 없습니다. 인증 기한 안에는 이 화면에서 다시 확인하세요. 접근 만료 후에도 이전 종료 결과는 확인되지 않은 상태입니다.");
    } finally {
      clearTimeout(deadline);
      if (attempt.current === controller) { attempt.current = null; setPending(false); }
    }
  }
  return <div className="browser-controls"><Button size="lg" className="browser-button" loading={pending} onClick={logout}>로그아웃</Button>
    <p role={pending || !notice ? "status" : "alert"} aria-live="polite" className="browser-notice">{notice}</p></div>;
}

declare global {
  interface Window { __consoleBrowserRestorationGuard?: { readonly blocked: boolean } }
}
export function PrivateBoundary({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    if (window.__consoleBrowserRestorationGuard?.blocked !== false) return;
    const region = document.getElementById("console-browser-private-region");
    const recovery = document.getElementById("console-browser-recovery");
    if (region && recovery) { region.hidden = false; recovery.hidden = true; }
  }, []);
  return <>
    <aside id="console-browser-recovery" className="browser-shell browser-card" aria-label="접근 확인 안내">
      <h1>접근 권한을 다시 확인합니다</h1>
      <p>개인 기록은 새 확인 뒤 표시됩니다. 자바스크립트(JavaScript)가 꺼져 있거나 스크립트를 불러오지 못하면 내용을 표시하지 않습니다.</p>
      <a href="/login/">다시 로그인</a>
    </aside>
    <div id="console-browser-private-region" hidden>{children}</div>
  </>;
}
