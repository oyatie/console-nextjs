// Pure seed helpers for work_orders.request_no fixtures.
//
// Hosted disposable-PG discovered two classes of fixture bugs late (~50m wall):
//   23505 unique (uuid % 1000 collides across tests on one DB)
//   23514 check  (EVD-{uuid} fails work_orders_request_no_check)
//
// Migration 0008_create_work_orders.sql:
//   request_no TEXT NOT NULL … CHECK (request_no ~ '^[0-9]{8}-[0-9]{3}$')
// Later: UNIQUE (org_id, request_no).
//
// This module is the local admission gate: no Docker, no network.

/** @type {RegExp} */
export const WORK_ORDER_REQUEST_NO_CHECK = /^[0-9]{8}-[0-9]{3}$/;

/**
 * Derive a CHECK-valid, high-entropy request_no from a work_order UUID.
 * Same shape as the docs evidence RLS fixture fix (#579).
 *
 * @param {string | bigint} uuidOrU128 UUID string or 128-bit integer
 * @returns {string}
 */
export function fixtureRequestNoFromWorkOrderId(uuidOrU128) {
  const n = typeof uuidOrU128 === "bigint" ? uuidOrU128 : uuidStringToU128(uuidOrU128);
  const left = Number(n % 100_000_000n);
  const right = Number((n / 100_000_000n) % 1000n);
  return `${String(left).padStart(8, "0")}-${String(right).padStart(3, "0")}`;
}

/**
 * @param {string} uuid
 * @returns {bigint}
 */
export function uuidStringToU128(uuid) {
  const hex = uuid.replace(/-/g, "");
  if (!/^[0-9a-fA-F]{32}$/.test(hex)) {
    throw new Error(`expected UUID hex, got ${uuid}`);
  }
  return BigInt(`0x${hex}`);
}

/**
 * Patterns that historically broke hosted PG and must not reappear in seeds.
 * @param {string} requestNo
 * @returns {string | null} rejection reason, or null if acceptable under CHECK only
 */
export function rejectKnownBadFixtureRequestNo(requestNo) {
  if (!WORK_ORDER_REQUEST_NO_CHECK.test(requestNo)) {
    return `fails work_orders_request_no_check ^[0-9]{8}-[0-9]{3}$: ${requestNo}`;
  }
  if (requestNo.startsWith("EVD-") || requestNo.startsWith("EVD")) {
    return `EVD-* prefix is not CHECK-valid: ${requestNo}`;
  }
  return null;
}
