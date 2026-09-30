#!/usr/bin/env node
// mox integration — slice 1 dev-harness (NOT a CI test).
//
// Proves the two live boundaries that a self-contained CI test cannot exercise
// without a running mox container + host callback:
//
//   1. SEND CONTRACT — POSTs the EXACT SendRequest shape our transport adapter
//      emits to the live dev mox webapi (`POST /webapi/v0/Send`, HTTP Basic auth)
//      and asserts mox accepts it and returns a Message-ID. This validates the
//      wire contract of `console-comms-adapter-mox` against a real mox.
//
//   2. WEBHOOK AUTH — drives our running backend's delivery webhook with a WRONG
//      secret (expect 401) and a well-formed UNKNOWN-recipient payload (expect
//      200 ingested=false), proving the auth gate + graceful ack on the live
//      HTTP surface.
//
// The seeded-recipient ingest + idempotency + notification path is proven in CI
// by `backend/crates/comms/rest/tests/mox_webhook.rs` (real console_rt, FORCE RLS).
// localserve does not auto-emit webhooks; production mox is configured with the
// account's IncomingWebhook {URL, Authorization}. Run `npm run dev` (or
// scripts/dev-up.mjs up) first.
//
// Dark cluster smoke (no public MX required): port-forward the internal services
// in two terminals, then run this script with explicit URLs and the webhook
// secret from OCI Vault/console-secrets. Do not print the secret.
//   kubectl -n console port-forward svc/console-mox 1080:1080
//   kubectl -n console port-forward svc/console-app 8090:8080
//   CONSOLE_MOX_WEBAPI_URL=http://127.0.0.1:1080 \
//   CONSOLE_DEV_BACKEND_URL=http://127.0.0.1:8090 \
//   CONSOLE_MOX_USER=postmaster@knllogistic.com \
//   CONSOLE_MOX_PASS=<from OCI Vault, not logged> \
//   CONSOLE_MAIL_MOX_WEBHOOK_SECRET=<from console-secrets, not logged> \
//   node scripts/mox-e2e.mjs
//
// Usage: node scripts/mox-e2e.mjs

const MOX = process.env.CONSOLE_MOX_WEBAPI_URL ?? "http://127.0.0.1:1080";
const MOX_USER = process.env.CONSOLE_MOX_USER ?? "mox@localhost";
const MOX_PASS = process.env.CONSOLE_MOX_PASS ?? "moxmoxmox";
const BACKEND = process.env.CONSOLE_DEV_BACKEND_URL ?? "http://127.0.0.1:8090";
const WEBHOOK_SECRET =
  process.env.CONSOLE_MAIL_MOX_WEBHOOK_SECRET ?? "mox-dev-webhook-secret-change-me";

let failures = 0;
function check(name, ok, detail) {
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
  if (!ok) failures += 1;
}

function basic(user, pass) {
  return `Basic ${Buffer.from(`${user}:${pass}`).toString("base64")}`;
}

async function sendViaMox() {
  // The identical JSON our adapter serializes (From/To/Subject/Text/References).
  const request = JSON.stringify({
    From: { Name: "Persona A", Address: MOX_USER },
    To: [{ Address: MOX_USER }],
    Subject: "mox slice-1 harness",
    Text: "sent through the mox webapi with our adapter's shape ☺",
  });
  const body = new URLSearchParams({ request }).toString();
  try {
    const res = await fetch(`${MOX}/webapi/v0/Send`, {
      method: "POST",
      headers: {
        Authorization: basic(MOX_USER, MOX_PASS),
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body,
    });
    const text = await res.text();
    let mid = null;
    try {
      mid = JSON.parse(text).MessageID;
    } catch {
      /* fall through to failure below */
    }
    check(
      "mox webapi accepts our SendRequest and returns a Message-ID",
      res.ok && typeof mid === "string" && mid.length > 0,
      res.ok ? mid : `HTTP ${res.status}: ${text.slice(0, 200)}`,
    );
  } catch (err) {
    check("mox webapi reachable at " + MOX, false, String(err));
  }
}

function incoming(rcpt) {
  return {
    Version: 0,
    From: [{ Name: "Persona A", Address: "persona-a@localhost" }],
    To: [{ Address: rcpt }],
    Subject: "delivery webhook harness",
    MessageID: `<harness-${Date.now()}@localhost>`,
    References: [],
    Text: "delivered",
    Meta: { MsgID: Date.now(), RcptTo: rcpt, MailboxName: "Inbox" },
  };
}

async function webhook(auth, payload) {
  const res = await fetch(`${BACKEND}/api/v1/mail/mox/webhook`, {
    method: "POST",
    headers: {
      ...(auth ? { Authorization: auth } : {}),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(payload),
  });
  const text = await res.text();
  return { status: res.status, text };
}

async function webhookAuth() {
  try {
    const wrong = await webhook("Bearer not-the-secret", incoming("x@localhost"));
    check("webhook rejects a wrong secret (401)", wrong.status === 401, `HTTP ${wrong.status}`);

    const unknown = await webhook(
      `Bearer ${WEBHOOK_SECRET}`,
      incoming("no-such-mailbox@localhost"),
    );
    const acked =
      unknown.status === 200 && /"ingested"\s*:\s*false/.test(unknown.text);
    check(
      "webhook acks an unknown recipient without ingesting (200 ingested=false)",
      acked,
      `HTTP ${unknown.status}: ${unknown.text.slice(0, 120)}`,
    );
  } catch (err) {
    check("backend webhook reachable at " + BACKEND, false, String(err));
  }
}

console.log("mox slice-1 dev-harness\n");
await sendViaMox();
await webhookAuth();
console.log(`\n${failures === 0 ? "ALL PASS" : `${failures} FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
