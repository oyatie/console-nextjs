# W3 POLISH-SWEEP — §4-25 closed-loop findings register (round 2)

**Method:** booted a real, isolated dev stack (scratch Postgres DB `console_w3polish`,
`console-app` built with `--features dev-auth` on port 18090, `vite` on port 15183 —
own DB/ports, no collision with other lanes), signed in as `SUPER_ADMIN` through
the real `RoleSwitcher` UI (`/api/v1/dev-auth/session`, a genuine signed
session — not a stub), completed the real passkey-onboarding consent gate, and
walked every one of the 34 `NAV_GROUPS` items in `web/src/console/shell/nav.ts`
with Playwright at 1440×900. Screenshots: `/private/tmp/claude-501/-Users-jasonlee-Developer-maintenance/e7ff00b7-8090-418f-aef4-967682538b8e/scratchpad/polish-r2/`.

No product code was changed by this lane (docs-only, per charter).

## Ranked findings

### 1. CRITICAL — the console's entire content-mount point is a dead P0.1 stub; every one of the 34 nav screens renders zero content
- **Screen:** all 34 (통합 개요, 인사 관리, 급여, 재무, 대시보드, 감사 로그, … every item in `NAV_GROUPS`).
- **Element:** `<section aria-label="화면 본문" data-cshell-screen={activeScreen}>` in `web/src/console/shell/ConsoleShell.tsx:202-206`.
- **Defect class:** layout/integration — mockup independence violation (§4-25-⑥), workflow coverage violation (§4-25-②).
- **Evidence:** `document.querySelector('[aria-label="화면 본문"]').childElementCount === 0` for every one of the 34 screens clicked (see `results.json` alongside the screenshots — every entry has `bodyChildCount: 0`, `bodyLabelText: ""`). Screenshots `00-overview.png` … `33-directory.png` all show sidebar+topbar chrome over a completely blank canvas. `git log --oneline -- web/src/console/shell/ConsoleShell.tsx` shows the file has not been touched since PR #249 (P0.1 chrome scaffold) / PR #269 (PolicyGated primitive) — no later wiring commit (Phase B1 wave 2, Phase C wave 1/2, fe-fix-wave1, BE-wire-2, etc., all referenced in `console-program-ledger.md`) ever touched this file. The `<section>`'s own inline comment still reads: *"Screens compose here in later slices; P0.1 renders the empty themed canvas for the active screen."*
- **Root cause:** the many screen-module directories that exist in the tree (`web/src/console/leave/`, `web/src/console/evidence/`, `web/src/console/dashboard/`, `web/src/console/finance/`, `web/src/console/ontology/`, etc. — 27 module directories total) and the ledger's "REALLY WIRED" / "gate green" entries describe those modules' own component/API correctness in isolation (and/or wiring into the *legacy* `/hr/leave-management`-style routes outside `/console/*`), but **none of them are ever mounted into this shell's screen slot**. The nav is fully interactive (buttons, `aria-current`, RUM route sampling all work) and correctly gated by role/feature (`visibleConsoleNav`), but clicking any item only ever swaps `data-cshell-screen`'s value — the DOM underneath never changes.
- **Impact:** the "carbon-copy Oyatie console" — the entire charter D1/M2/W2 deliverable — is not reachable through its own shell today. Every other §4-25 question (workflow coverage, friction, benchmark, persona coverage, layout/margins) is moot for the 34 screens until this is fixed; this finding should gate the next wave, not be queued behind cosmetic polish.
- **Severity:** CRITICAL — blocks the console's core purpose.

### 2. HIGH — command palette (⌘K) opens but never returns results for any query
- **Screen:** global (all screens — the palette is shell-level chrome).
- **Element:** `<div data-cshell-palette-results style={{ minHeight: 96 }} />` in `ConsoleShell.tsx:326`.
- **Defect class:** interaction — dead-end affordance, missing loading/empty/result states (§4-10).
- **Evidence:** `chrome-palette.png` — palette opens on ⌘K with a focused, fully-functional-looking search input ("사람·업무·문서 검색") and an `Esc` chip, but the results panel below is permanently empty regardless of whether anything is typed. The code comment confirms: *"Empty results surface — the full palette (result rows, keyboard nav, run handlers) is a later slice."* This matches §4-10 (empty state must carry a reason + next action) — here there is no reason surfaced at all, empty or typed.
- **Impact:** a competent-looking global search invites use and then silently does nothing — the single worst kind of dead end for user trust.
- **Severity:** HIGH.

### 3. MEDIUM — scope selector is a single-option dead end even for SUPER_ADMIN
- **Screen:** global (topbar scope switcher, all screens).
- **Element:** `그룹 전체` dropdown, `Topbar` component (rendered via `useConsoleScopes`).
- **Defect class:** interaction/workflow coverage (§4-25-②) — the org/branch drill affordance that the UI presents (a chevron dropdown) has nothing to drill into.
- **Evidence:** `chrome-scope-dropdown.png` — opening the dropdown for a `SUPER_ADMIN` session (which should see every branch/site in the org per `BranchScope::All`) shows exactly one row, "그룹 전체", with a checkmark. No branch/site list appears beneath it.
- **Impact:** the scope selector implies drill-down scoping (branch/site/region) is available; today it is cosmetically present but functionally a no-op for every role.
- **Severity:** MEDIUM.

### 4. MEDIUM — collapsed sidebar loses all group separation, becomes an undifferentiated icon stack
- **Screen:** global (sidebar, collapsed state — reachable from every screen via the collapse toggle).
- **Element:** `Sidebar.tsx` collapsed rendering (icon-only list, `[data-collapsed="true"]`).
- **Defect class:** layout/ergonomics (§4-25-①, §4-25-⑧ — 8px-grid/gap consistency + scannability).
- **Evidence:** `chrome-collapsed.png` — the expanded sidebar clearly chunks 9 groups with uppercase group labels + vertical gaps (`overview.png` etc.); collapsed, all groups run together as one unbroken column of ~27 icons with no divider rule, no extra gap, and no group-boundary affordance (only a `title` tooltip per icon, one at a time, on hover). A SUPER_ADMIN scanning for "governance" icons among "ERP" icons has no visual landmark once collapsed.
- **Impact:** collapsing the sidebar — the exact action a user takes to reclaim screen width when they already know where they're going — actively removes the only wayfinding aid (group headers) without replacing it with anything (a thin divider between groups would suffice).
- **Severity:** MEDIUM.

### 5. MEDIUM — manual sidebar expand overrides the responsive auto-collapse permanently, with no narrow-viewport fallback
- **Screen:** global (shell chrome), reproduced at 768px and 1024px viewport widths.
- **Element:** `sbUser` state in `ConsoleShell.tsx:49,68` (`const collapsed = sbUser ?? narrow`), `Sidebar` fixed 236px width.
- **Defect class:** layout/interaction (§4-25-⑧ — layout doesn't degrade to available viewport; §4-25-③ friction).
- **Evidence:** `chrome-narrow-768.png` — after a user has ever manually toggled the sidebar open (`sbUser` becomes a non-null boolean), the `matchMedia("(max-width: 1279px)")` auto-collapse effect is permanently shadowed by `sbUser`. At a 768px viewport the full 236px labeled sidebar still renders, leaving only ~530px for the (already-empty, see Finding 1) content canvas, and there is no hamburger/drawer/overlay fallback for narrow widths — the layout simply degrades in place.
- **Impact:** on a laptop with a docked side panel, a tablet, or any window narrower than ~1280px, one manual re-expand permanently defeats the responsive behavior for the rest of the session.
- **Severity:** MEDIUM.

### 6. LOW — passkey-onboarding step repeats the exact same string as both a static status chip and the primary CTA
- **Screen:** `/onboarding` (passkey registration step, reached by any freshly-provisioned persona — every real first-login user, not just dev-auth).
- **Element:** header chip "필수 동의 후 계속" and the primary submit `<button>` "필수 동의 후 계속" on the same screen.
- **Defect class:** §4-12 (no explanatory/redundant UI — every string should carry unique information).
- **Evidence:** `debug-login-after.png` — the header row shows two step chips ("필수 개인정보 수집·이용 및 약관 동의" done, "패스키 등록" pending) plus, immediately to their right, a THIRD pill reading "필수 동의 후 계속" that duplicates the CTA button's own label verbatim, with no distinct function (it's not clickable, not a third step).
- **Impact:** minor, but a §4-12 violation — the duplicate string adds visual noise without adding information.
- **Severity:** LOW.

### 7. LOW — comms rail is fully decorative; icons imply interactivity they don't have
- **Screen:** global (right-hand 54px rail, all screens).
- **Element:** `RailGlyph` (`msg`/`mail`/`bell`) in `ConsoleShell.tsx:240-242`.
- **Defect class:** interaction — affordance mismatch (icons render with hover-capable styling but no `onClick`).
- **Evidence:** code comment confirms intent: *"Comms rail — collapsed strip only (chrome). The interactive rail (open views: messenger/mail/notif) arrives in P2 … presentational here: no unwired handlers."* Visually indistinguishable from a working icon rail (see any of the 34 screenshots, right edge).
- **Impact:** users will click expecting a messenger/mail/notification flyout (exactly what the icons denote) and get nothing.
- **Severity:** LOW (explicitly documented as a future slice in-code, but still a live dead-end affordance today).

## Not re-litigated here
Round-1 verdict findings already on record in `console-program-ledger.md` (§"Visual-verdict round 1") — module-finance placeholder chips, dashboard legacy-page captions, big-number KPI tiles on support, stale-backend ontology/explore/automate/policy-canvas 404s — could not be independently re-verified this round because Finding 1 means none of those screens render any content at all under the current shell; they are superseded by Finding 1 until it is fixed, at which point a recapture is needed to re-assess them individually.
