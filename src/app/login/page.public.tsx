import { LoginControl } from "@/components/browser-session/SessionControls";

export const dynamic = "force-dynamic";
export default function LoginPage() {
  return <main className="browser-shell browser-card">
    <a href="/">Oyatie</a>
    <h1>패스키 로그인</h1>
    <p>등록한 패스키로 본인의 근태 기록을 확인하세요. 이 기기에 저장되었거나 동기화된 검색 가능한 패스키가 필요합니다.</p>
    <p>인증은 원래 기한까지 유효하며 자동 연장되지 않습니다. 이 페이지는 다른 화면의 로그인 상태를 대신 선택하지 않습니다.</p>
    <LoginControl />
    <p className="browser-muted">접근 정보가 없거나 만료되었다면 새로 로그인하세요. 쿠키가 사라져도 이전 종료 결과는 확인되지 않은 상태입니다.</p>
    <noscript><p>로그인에는 자바스크립트(JavaScript)가 필요합니다. 브라우저 설정을 확인하세요.</p></noscript>
  </main>;
}
