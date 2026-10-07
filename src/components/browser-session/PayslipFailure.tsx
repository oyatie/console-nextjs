export function PayslipFailure({ status, retry, home }: { status: number; retry: string; home: string }) {
  const denied = status === 401 || status === 403 || status === 410;
  const missing = status === 404;
  return <main className="browser-shell browser-card">
    <h1>{denied ? "접근 정보를 확인할 수 없습니다" : missing ? "명세서를 찾을 수 없습니다" : "명세서를 불러올 수 없습니다"}</h1>
    <p role="alert">{denied ? "접근 권한이 유효하지 않거나 인증이 만료되었습니다. 다시 로그인하세요. 접근 만료만으로 이전 로그아웃 결과가 확인되지는 않습니다." :
      missing ? "현재 권한으로 조회할 수 있는 급여명세서가 없습니다. 목록에서 다시 선택하세요." :
      "서버 연결 또는 명세서 형식을 확인할 수 없습니다. 이전 자료나 임의의 금액을 표시하지 않습니다."}</p>
    <a href={denied ? "/login/" : missing ? home : retry}>{denied ? "다시 로그인" : missing ? "명세서 목록으로 이동" : "다시 불러오기"}</a>
  </main>;
}
