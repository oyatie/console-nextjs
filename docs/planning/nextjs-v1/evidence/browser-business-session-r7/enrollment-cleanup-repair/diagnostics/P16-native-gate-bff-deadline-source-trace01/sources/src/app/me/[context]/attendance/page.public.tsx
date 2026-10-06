import { ZodError } from "zod";
import { headers } from "next/headers";
import { LogoutControl, PrivateBoundary } from "@/components/browser-session/SessionControls";
import { BrowserSessionError, canonicalUuid, formatAttendanceInstant, ownAttendance } from "@/lib/server/browser-session";

export const dynamic = "force-dynamic";
const kinds = { CLOCK_IN: "출근", OUT_FOR_WORK: "외근", BUSINESS_TRIP: "출장", RETURNED: "복귀", CLOCK_OUT: "퇴근" };
const states = { CLOCKED_IN: "근무 중", OUT_FOR_WORK: "외근 중", BUSINESS_TRIP: "출장 중", OFF_DUTY: "근무 종료" };

export default async function AttendancePage({ params, searchParams }: {
  params: Promise<{ context: string }>; searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  let result: Awaited<ReturnType<typeof ownAttendance>>;
  let context: string | undefined;
  let page = 1;
  let inputValidated = false;
  try {
    context = canonicalUuid.parse((await params).context);
    const query = await searchParams;
    if (Object.keys(query).some((key) => key !== "page")) throw new BrowserSessionError(400);
    const selected = query.page ?? "1";
    if (typeof selected !== "string" || !/^[1-9][0-9]*$/.test(selected)) throw new BrowserSessionError(400);
    page = Number(selected);
    const offset = (page - 1) * 25;
    if (!Number.isSafeInteger(page) || !Number.isSafeInteger(offset)) throw new BrowserSessionError(400);
    inputValidated = true;
    result = await ownAttendance(new Headers(await headers()), context, offset);
  } catch (error) {
    const status = error instanceof BrowserSessionError ? error.status :
      error instanceof ZodError && !inputValidated ? 400 : 503;
    if (!inputValidated && status === 400) return <main className="browser-shell browser-card"><h1>입력 정보를 확인하세요</h1>
      <p role="alert">페이지 번호 또는 주소가 올바르지 않습니다. 페이지 번호는 1 이상의 정수여야 합니다.</p>
      {context ? <a href={`/me/${context}/attendance/`}>처음 페이지로 이동</a> : <a href="/login/">로그인 화면으로 이동</a>}</main>;
    const denied = status === 401 || status === 403 || status === 410;
    return <main className="browser-shell browser-card"><h1>{denied ? "접근 정보를 확인할 수 없습니다" : "기록을 불러올 수 없습니다"}</h1>
      <p role="alert">{denied ?
        "접근 권한이 유효하지 않거나 인증이 만료되었습니다. 다시 로그인하세요. 접근 정보가 없어도 이전 종료 결과는 확인되지 않은 상태입니다." :
        "서버 연결 또는 응답을 확인할 수 없습니다. 이전 기록을 현재 결과로 표시하지 않습니다. 다시 시도하세요."}</p>
      {!denied && inputValidated && context ?
        <a href={`/me/${context}/attendance/${page > 1 ? `?page=${page}` : ""}`}>다시 불러오기</a> :
        <a href="/login/">다시 로그인</a>}</main>;
  }
  const { data, csrf } = result;
  const { history } = data;
  const first = history.items.length ? history.offset + 1 : 0;
  const last = history.items.length ? history.offset + history.items.length : 0;
  const path = `/me/${context}/attendance/`;
  return <PrivateBoundary><main className="browser-shell">
    <header className="browser-header"><div><a href="/">Oyatie</a><h1>나의 근태 기록</h1></div><LogoutControl context={context} csrf={csrf} /></header>
    <section className="browser-card" aria-label="현재 조회 정보">
      <dl className="browser-context"><div><dt>회사</dt><dd>{data.context.company_name}</dd></div>
        <div><dt>계정</dt><dd>{data.context.account_display_name}</dd></div>
        <div><dt>인증 기한</dt><dd><time dateTime={data.expires_at}>{new Intl.DateTimeFormat("ko-KR", { timeZone: "Asia/Seoul", dateStyle: "medium", timeStyle: "medium" }).format(new Date(data.expires_at))} (KST)</time></dd></div></dl>
      <p className="browser-muted">인증은 자동 연장되지 않습니다. 기한이 지나면 패스키로 다시 로그인하세요.</p>
    </section>
    <section className="browser-card"><h2>기록 내역</h2>
      <p>총 {history.total}건 · {first}–{last}건</p>
      <p className="browser-muted">한국 표준시(KST), 초 단위로 표시합니다. 소수 초는 표시하지 않습니다. 비고는 본인 또는 직원이 입력한 기록 메모입니다.</p>
      <p className="browser-muted">급여 자료 연결은 계산·승인·지급을 의미하지 않습니다.</p>
      {!data.context.employee_linked ? <p role="status">계정에 연결된 직원 정보가 없습니다. 연결 확인이 필요합니다.</p> :
        history.total === 0 ? <p role="status">근태 기록이 없습니다.</p> :
        history.items.length === 0 ? <p role="status">요청한 페이지가 전체 기록 범위를 벗어났습니다. 처음 페이지로 이동하세요.</p> : null}
      <div role="region" aria-label="근태 기록 표" tabIndex={0} className="browser-table-region">
        <table aria-label="근태 기록"><thead><tr><th scope="col">근무일</th><th scope="col">기록 시각</th><th scope="col">기록 종류</th><th scope="col">기록 후 상태</th><th scope="col">비고</th><th scope="col">급여 자료</th></tr></thead>
          <tbody>{history.items.map((row) => <tr key={row.id}><td>{row.work_date}</td><td><time>{formatAttendanceInstant(row.occurred_at)}</time></td>
            <td>{kinds[row.kind]}</td><td>{states[row.state_after]}</td><td>{row.note ?? "메모 없음"}</td><td>{row.payroll_link_status === "LINKED" ? "자료 연결됨" : "자료 미연결"}</td></tr>)}</tbody></table>
      </div>
      <nav aria-label="근태 기록 페이지" className="browser-pagination">
        {page > 1 ? <><a href={path}>처음</a><a href={`${path}?page=${page - 1}`}>이전</a></> : <span>처음</span>}
        <span>{page}페이지</span>
        {history.offset + history.items.length < history.total && Number.isSafeInteger(page + 1) && Number.isSafeInteger(page * 25) ? <a href={`${path}?page=${page + 1}`}>다음</a> : <span>다음 없음</span>}
      </nav>
    </section>
  </main></PrivateBoundary>;
}
