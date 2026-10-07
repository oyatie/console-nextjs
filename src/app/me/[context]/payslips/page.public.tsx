import { headers } from "next/headers";
import { LogoutControl, PrivateBoundary } from "@/components/browser-session/SessionControls";
import { PayslipFailure } from "@/components/browser-session/PayslipFailure";
import { BrowserSessionError, canonicalUuid } from "@/lib/server/browser-session";
import { formatIssuedAt, ownPayslipList } from "@/lib/server/payslips";

export const dynamic = "force-dynamic";
export default async function PayslipsPage({ params, searchParams }: {
  params: Promise<{ context: string }>; searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  let context: string;
  let before: string | undefined;
  try {
    context = canonicalUuid.parse((await params).context);
    const query = await searchParams;
    if (Object.keys(query).some((key) => key !== "before") || Array.isArray(query.before)) throw new Error();
    before = query.before === undefined ? undefined : canonicalUuid.parse(query.before);
  } catch { return <main className="browser-shell browser-card"><h1>입력 정보를 확인하세요</h1>
    <p role="alert">주소 또는 페이지 기준이 올바르지 않습니다.</p><a href="/login/">로그인 화면으로 이동</a></main>; }
  const home = `/me/${context}/payslips/`;
  const path = before ? `${home}?before=${before}` : home;
  let result;
  try { result = await ownPayslipList(new Headers(await headers()), context, before); }
  catch (error) { return <PayslipFailure status={error instanceof BrowserSessionError ? error.status : 503} retry={path} home={home} />; }
  const { data, csrf } = result;
  return <PrivateBoundary><main className="browser-shell">
    <header className="browser-header"><div><a href="/">Oyatie</a><h1>나의 급여명세서</h1></div><LogoutControl context={context} csrf={csrf} /></header>
    <nav aria-label="나의 기록" className="browser-pagination"><a href={`/me/${context}/attendance/`}>근태 기록</a><span aria-current="page">급여명세서</span></nav>
    <section className="browser-card">
      <p>조회 회사 ID: {data.company_id}</p>
      <p>발급된 명세서를 조회합니다. 명세서 열람은 수령 동의나 은행 이체 확인을 의미하지 않습니다.</p>
      {data.items.length === 0 ? <p role="status">{before ? "이 기준보다 오래된 급여명세서가 없습니다. 처음 목록에서 다시 확인하세요." : "발급된 급여명세서가 없습니다."}</p> :
        <ul className="browser-document-list">{data.items.map((row) => <li key={row.id}>
          <a href={`${home}${row.id}/${before ? `?from=${before}` : ""}`}>{row.title}</a>
          <p className="browser-muted">기록된 발급 시각: <time dateTime={row.created_at}>{formatIssuedAt(row.created_at)} (KST)</time></p>
        </li>)}</ul>}
      <nav aria-label="급여명세서 페이지" className="browser-pagination">
        {before ? <a href={home}>처음 목록</a> : <span>처음 목록</span>}
        {data.next_cursor ? <a href={`${home}?before=${data.next_cursor}`}>이전 명세서</a> : <span>마지막 목록</span>}
        <a href={path}>다시 불러오기</a>
      </nav>
    </section>
  </main></PrivateBoundary>;
}
