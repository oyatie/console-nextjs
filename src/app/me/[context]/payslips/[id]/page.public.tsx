import { headers } from "next/headers";
import { LogoutControl, PrivateBoundary } from "@/components/browser-session/SessionControls";
import { PayslipFailure } from "@/components/browser-session/PayslipFailure";
import { BrowserSessionError, canonicalUuid } from "@/lib/server/browser-session";
import { formatIssuedAt, formatWon, ownPayslip } from "@/lib/server/payslips";

export const dynamic = "force-dynamic";
export default async function PayslipPage({ params, searchParams }: {
  params: Promise<{ context: string; id: string }>; searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  let context: string; let id: string; let from: string | undefined;
  try {
    const route = await params;
    context = canonicalUuid.parse(route.context); id = canonicalUuid.parse(route.id);
    const query = await searchParams;
    if (Object.keys(query).some((key) => key !== "from") || Array.isArray(query.from)) throw new Error();
    from = query.from === undefined ? undefined : canonicalUuid.parse(query.from);
  } catch { return <main className="browser-shell browser-card"><h1>입력 정보를 확인하세요</h1>
    <p role="alert">주소 또는 돌아갈 목록 기준이 올바르지 않습니다.</p><a href="/login/">로그인 화면으로 이동</a></main>; }
  const home = `/me/${context}/payslips/${from ? `?before=${from}` : ""}`;
  const path = `/me/${context}/payslips/${id}/${from ? `?from=${from}` : ""}`;
  let result;
  try { result = await ownPayslip(new Headers(await headers()), context, id); }
  catch (error) { return <PayslipFailure status={error instanceof BrowserSessionError ? error.status : 503} retry={path} home={home} />; }
  const { data, csrf } = result;
  const { document } = data;
  const pay = document.payload;
  return <PrivateBoundary><main className="browser-shell">
    <header className="browser-header"><div><a href={home}>명세서 목록으로 돌아가기</a><h1>{document.title}</h1></div><LogoutControl context={context} csrf={csrf} /></header>
    <section className="browser-card" aria-label="급여명세서 금액">
      <p>조회 회사 ID: {data.company_id}</p>
      <p>산정 기간: <time dateTime={pay.period_start}>{pay.period_start}</time> ~ <time dateTime={pay.period_end}>{pay.period_end}</time></p>
      <p>기록된 발급 시각: <time dateTime={document.created_at}>{formatIssuedAt(document.created_at)} (KST)</time></p>
      <dl className="browser-context"><div><dt>총 지급액</dt><dd>{formatWon(pay.gross_won)}원</dd></div>
        <div><dt>공제 합계</dt><dd>{formatWon(pay.total_deductions_won)}원</dd></div>
        <div><dt>명세서상 차감 후 금액</dt><dd>{formatWon(pay.net_won)}원</dd></div></dl>
      <p className="browser-muted">명세서 금액은 은행 이체의 접수·완료 여부를 증명하지 않습니다.</p>
    </section>
    <section className="browser-card"><h2>공제 내역</h2>
      {pay.deductions.length === 0 ? <p>이 명세서에 기록된 공제 항목이 없습니다.</p> :
        <div role="region" aria-label="공제 내역 표" tabIndex={0} className="browser-table-region browser-deductions">
          <table><caption className="sr-only">명세서에 기록된 공제 항목, 원 단위 금액 및 출처</caption>
            <thead><tr><th scope="col">항목</th><th scope="col">금액 (원)</th><th scope="col">기록된 출처</th></tr></thead>
            <tbody>{pay.deductions.map((row) => <tr key={row.code}><th scope="row">{row.label_ko}</th>
              <td>{formatWon(row.amount_won)}</td><td>{row.source_url}</td></tr>)}</tbody></table>
        </div>}
    </section>
    <section className="browser-card"><h2>발급 근거</h2>
      <dl><dt>세액표 버전</dt><dd>{pay.tax_table_version}</dd><dt>계산 버전</dt><dd>{pay.calculation_version}</dd>
        <dt>급여 실행 ID</dt><dd>{pay.run_id}</dd><dt>산정 행 ID</dt><dd>{pay.line_id}</dd><dt>명세서 ID</dt><dd>{document.id}</dd></dl>
      <p>금액이나 기간 확인을 요청할 때 명세서 ID와 계산 버전을 함께 전달하세요. 발급된 원본을 이 화면에서 변경하지 않습니다.</p>
      <a href={path}>다시 불러오기</a>
    </section>
  </main></PrivateBoundary>;
}
