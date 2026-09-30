> **SUPERSEDED PRE-PIVOT SPEC:** This frozen record depends on deleted Android,
> iOS, and generated TypeScript/Kotlin/Swift clients. It is not current product
> scope, implementation authority, or a runnable command guide. Current authority
> is [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md).

# Mobile passkey step-up binding contract

This contract defines the narrow API and native-client shape needed for issue #407: Android and iOS approval, poll-vote, and queued replay flows must obtain a real passkey assertion and bind it to the sensitive action it authorizes.

## Decision summary

- Do not overload `POST /api/v1/auth/passkey/login/start` for mobile step-up. It is a pre-auth, usernameless login ceremony with no action binding. Mobile sensitive actions need an authenticated, action-bound start route.
- Add a generated OpenAPI contract first, then regenerate TypeScript, Kotlin, and Swift clients. Do not hand-edit `clients/ts`, `clients/kotlin`, or `clients/swift`.
- Keep the existing `PasskeyStepUpAssertion` shape for already-shipped web/register/workflow/object-action callers. Add a mobile-specific envelope that pairs that assertion with an explicit binding.
- Each action attempt and each queued replay attempt obtains a fresh ceremony and assertion. Native clients must never persist or replay a credential assertion; they may persist only action payload and replay-attempt metadata.

## Existing surfaces this builds on

- Existing start ceremony: `POST /api/v1/auth/passkey/login/start` returns `PasskeyLoginStartResponse { ceremony_id, challenge, expires_at }`. The challenge is serialized WebAuthn `RequestChallengeResponse` JSON and may be wrapped under `publicKey`; clients must feed the public-key request options into Credential Manager / AuthenticationServices. This route remains a precedent for WebAuthn challenge shape only; mobile action binding must use the authenticated step-up start route below.
- Existing assertion proof: `PasskeyStepUpAssertion { ceremony_id, credential }`, where `credential` is the platform WebAuthn assertion JSON. When the OpenAPI description is regenerated, it must not imply mobile callers obtained this `ceremony_id` from the pre-auth login start route; mobile step-up assertions come from `POST /api/v1/auth/passkey/step-up/start`.
- Existing verifier: `PasskeyService::verify_step_up_for_user(pool, ceremony_id, credential, expected_user_id)` claims an authentication ceremony once, rejects expired/consumed ceremonies, verifies user verification, and rejects credentials not owned by the authenticated user.
- Existing mobile placeholders:
  - Android `MobileOperationsRepository.approveWorkOrder(... stepUpAssertion: PasskeyStepUpAssertion?)` queues when null and discards non-null assertions before calling `approveWorkOrder`.
  - Android poll voting creates `stepUpEnvelope(...)` and ignores it before `votePoll`.
  - Android replay accepts one nullable assertion for all queued actions.
  - iOS mirrors these placeholder/null paths in `ConsoleViewModel.swift` and `MobileOperationsRepository.swift`.

## Generated OpenAPI contract

Add these schemas to `backend/openapi/openapi.yaml` and regenerate clients through the existing scripts (`npm run gen:api:portable`, `npm run gen:api:swift`) in the backend/API implementation lane.

```yaml
MobileStepUpActionKind:
  type: string
  enum:
    - APPROVAL_DECISION
    - POLL_VOTE

MobilePasskeyStepUpBinding:
  type: object
  required:
    - action_kind
    - object_id
    - reason_key
    - replay_attempt
  properties:
    action_kind:
      $ref: '#/components/schemas/MobileStepUpActionKind'
    object_id:
      $ref: '#/components/schemas/Uuid'
    reason_key:
      type: string
      enum:
        - operations_passkey_approval_decision
        - operations_passkey_poll_vote
    replay_attempt:
      type:
        - integer
        - 'null'
      format: int32
      minimum: 1
      description: Null for an immediate online action; 1-based for queued replay attempts.

MobilePasskeyStepUpStartRequest:
  type: object
  required:
    - binding
  properties:
    binding:
      $ref: '#/components/schemas/MobilePasskeyStepUpBinding'

MobilePasskeyStepUpStartResponse:
  type: object
  required:
    - ceremony_id
    - challenge
    - expires_at
    - binding
  properties:
    ceremony_id:
      $ref: '#/components/schemas/Uuid'
    challenge:
      type: object
      additionalProperties: true
    expires_at:
      $ref: '#/components/schemas/Timestamp'
    binding:
      $ref: '#/components/schemas/MobilePasskeyStepUpBinding'

MobilePasskeyStepUpEnvelope:
  type: object
  required:
    - binding
    - assertion
  properties:
    binding:
      $ref: '#/components/schemas/MobilePasskeyStepUpBinding'
    assertion:
      $ref: '#/components/schemas/PasskeyStepUpAssertion'
```

Add an authenticated start endpoint:

```http
POST /api/v1/auth/passkey/step-up/start
Authorization: Bearer <access-token>
Content-Type: application/json

{
  "binding": {
    "action_kind": "APPROVAL_DECISION",
    "object_id": "4c313d79-28dd-4383-83c5-4b8c0e7e0a6d",
    "reason_key": "operations_passkey_approval_decision",
    "replay_attempt": null
  }
}
```

Response:

```json
{
  "ceremony_id": "bca0c6ae-6e98-4111-9d6a-7d8ef70bc998",
  "challenge": { "publicKey": { "challenge": "...", "allowCredentials": [] } },
  "expires_at": "2026-07-09T10:20:00Z",
  "binding": {
    "action_kind": "APPROVAL_DECISION",
    "object_id": "4c313d79-28dd-4383-83c5-4b8c0e7e0a6d",
    "reason_key": "operations_passkey_approval_decision",
    "replay_attempt": null
  }
}
```

The server must persist the binding keyed by `ceremony_id` outside the raw WebAuthn `state_json` (for example a side table keyed to `auth_webauthn_ceremonies.id`, or an append-only migration adding dedicated binding columns). Do not wrap/alter the serialized `DiscoverableAuthentication` state unless the finish path is changed and tested to deserialize it safely.

## Action request shapes

The safe route choice for this contract is mobile-scoped action endpoints. Existing web endpoints may continue to accept their current request bodies until web callers are intentionally migrated; native mobile clients must call the generated mobile endpoints below so their sensitive actions cannot fall through a bearer-token path with `step_up = null`.

### Approval decision

Endpoint: `POST /api/v1/mobile/work-orders/{workOrderId}/approve`.

Request body after contract change:

```json
{
  "comment": "승인합니다.",
  "step_up": {
    "binding": {
      "action_kind": "APPROVAL_DECISION",
      "object_id": "4c313d79-28dd-4383-83c5-4b8c0e7e0a6d",
      "reason_key": "operations_passkey_approval_decision",
      "replay_attempt": null
    },
    "assertion": {
      "ceremony_id": "bca0c6ae-6e98-4111-9d6a-7d8ef70bc998",
      "credential": { "id": "...", "rawId": "...", "type": "public-key", "response": { } }
    }
  }
}
```

Server expectations:

1. Compute the expected binding from the authenticated user, mobile route path, and request: `APPROVAL_DECISION`, `object_id = workOrderId`, `reason_key = operations_passkey_approval_decision`, and the submitted `replay_attempt`.
2. Require `step_up.binding` to equal the expected binding.
3. Require the persisted ceremony binding for `step_up.assertion.ceremony_id` to equal the expected binding.
4. Verify `step_up.assertion` for the authenticated user with user verification required.
5. Only after all checks pass, call the existing `approve_work_order` mutation/audit path.

### Poll vote

Endpoint: `POST /api/v1/mobile/collaboration/polls/{id}/vote`.

Request body after contract change:

```json
{
  "selected_option_ids": ["88038ce1-fb69-4690-870d-0cafb45000be"],
  "step_up": {
    "binding": {
      "action_kind": "POLL_VOTE",
      "object_id": "99e492cc-0381-4cca-9285-4d8fd4d8847f",
      "reason_key": "operations_passkey_poll_vote",
      "replay_attempt": null
    },
    "assertion": {
      "ceremony_id": "dd61cc56-2f86-4077-bfe3-c0d5aca1a201",
      "credential": { "id": "...", "rawId": "...", "type": "public-key", "response": { } }
    }
  }
}
```

Server expectations mirror approval: mobile route path `id` is the bound `object_id`, `action_kind = POLL_VOTE`, `reason_key = operations_passkey_poll_vote`, binding must match both the request and the persisted ceremony binding, then the existing poll lifecycle, validation, upsert, lifecycle-event, and audit path runs.

### Queued replay

Queued replay uses the original mobile-scoped endpoint for the queued action; it does not send sensitive approval/poll votes through `/api/v1/sync` unless a future generated sync-operation schema adds the same per-operation `MobilePasskeyStepUpEnvelope`.

For every queued sensitive action, the mobile store must persist:

- `action_kind`
- `object_id`
- `reason_key`
- original payload needed for replay (`comment` for approval; `selected_option_ids` for poll vote)
- `next_replay_attempt`, initially `1`

Before each replay attempt:

1. Build binding with the queued action fields and `replay_attempt = next_replay_attempt`.
2. Call `POST /api/v1/auth/passkey/step-up/start` with that binding.
3. Run the native passkey ceremony against the returned challenge.
4. Submit the mobile-scoped endpoint with `step_up.binding.replay_attempt = next_replay_attempt`.
5. If the server accepts and the mutation completes, mark the queued action submitted.
6. If the user cancels/no credential is available, keep or move the action to `WAITING_FOR_PASSKEY`; do not increment the attempt.
7. If transport/server submission fails after a fresh assertion was produced, keep the action replayable, increment `next_replay_attempt`, and discard the assertion.

The assertion is never stored. A replay attempt always means a new start ceremony, a new platform assertion, and a single submit.

## Failure behavior

- Missing/null `step_up` on a mobile-sensitive approval or poll request: fail closed with HTTP 428 and error code `passkey_step_up_required` before any mutation/audit side effect.
- Malformed binding (unknown action kind, missing object id, invalid replay attempt, unsupported reason key): HTTP 422 validation error.
- Expired ceremony, already-consumed ceremony, stale/replayed assertion, credential not owned by the authenticated user, user verification missing, or WebAuthn verification failure: HTTP 401 with error code `passkey_step_up_failed`.
- Binding mismatch between endpoint-derived expected binding, request `step_up.binding`, and persisted ceremony binding: HTTP 401 with error code `passkey_step_up_binding_mismatch`.
- Passkey verifier not configured on a route that requires it: HTTP 503, matching existing registry object-action behavior.

Client behavior:

- Android/iOS must not call `approveWorkOrder` or `votePoll` with a null assertion for an immediate action.
- Android/iOS must not pass one assertion to `replaySensitiveActions` for multiple queued items. Replay is per queued item, per attempt.
- `WAITING_FOR_PASSKEY` means user/passkey input is still required and no server mutation was attempted. Transition out of it only after a fresh ceremony+assertion exists for that specific queued action attempt.
- `READY_FOR_REPLAY` may hold only the replay payload and metadata, never the assertion.

## Operations, audit, rollback, and observability

- Audit records for successful approval and poll mutations must continue through the existing mutation-specific audit paths after step-up verification succeeds. Failed step-up attempts may emit security/observability events with actor, action kind, object id, reason key, replay attempt, request id, ceremony id, and stable error code, but must not log raw WebAuthn challenges, credentials/assertions, comments, poll option labels, or other payload bytes.
- Metrics/traces should distinguish start, verification success, `passkey_step_up_required`, `passkey_step_up_failed`, `passkey_step_up_binding_mismatch`, and verifier/configuration failures so rollout can detect native-client fallback or replay loops.
- Rollback must fail closed. If the mobile step-up route or verifier is disabled, native clients queue or leave actions in `WAITING_FOR_PASSKEY`; they must not fall back to the legacy non-mobile approval or poll endpoints with a null assertion.

## Native implementation touchpoints

Android:

- `android/app/src/main/kotlin/com/console/app/auth/PasskeyCredentialClient.kt`: existing `getLoginCredential(context, challengeJson)` can be generalized/renamed or wrapped as `getStepUpCredential` because the challenge/assertion JSON shape is the same.
- `android/app/src/main/kotlin/com/console/app/auth/PasskeyAuthRepository.kt`: keep login as-is; expose a small step-up service that calls the new generated `startMobilePasskeyStepUp` operation and returns `MobilePasskeyStepUpEnvelope`.
- `android/app/src/main/kotlin/com/console/app/data/api/ConsoleApiGateway.kt`: generated calls must use the mobile-scoped approval and poll-vote operations and pass `step_up`.
- `android/app/src/main/kotlin/com/console/app/data/collaboration/MobileOperationsRepository.kt`: replace nullable `PasskeyStepUpAssertion?` parameters with the generated `MobilePasskeyStepUpEnvelope`, add poll replay payload and `nextReplayAttempt`, and make replay per action.
- `android/app/src/main/kotlin/com/console/app/ui/ConsoleApp.kt`: request a fresh step-up envelope before approval, poll vote, and each queued replay item.

iOS:

- `ios/Sources/ConsoleCore/AuthRepository.swift`: keep login as-is; add a step-up repository/service that calls the new generated start operation and returns `MobilePasskeyStepUpEnvelope`.
- `ios/Sources/ConsoleApp/AuthorizationPasskeyCredentialProvider.swift`: existing `credentialAssertion(challengeJSON:)` can be reused for step-up because the native assertion JSON is the same. Ensure it unwraps the `publicKey.challenge` form used by WebAuthn `RequestChallengeResponse`.
- `ios/Sources/ConsoleCore/MobileOperationsRepository.swift`: mirror Android queue metadata, generated envelope, and per-action replay behavior.
- `ios/Sources/ConsoleApp/ConsoleViewModel.swift`: replace placeholder `stepUpEnvelope(...)` calls and nil `stepUpAssertion` arguments with a real ceremony before approval, poll vote, and replay.

Backend/API:

- `backend/openapi/openapi.yaml`: add the schemas and request/response fields first.
- `backend/crates/platform/auth-rest/src/lib.rs`: add `POST /api/v1/auth/passkey/step-up/start`, authenticated through the bearer token, and persist binding for the ceremony.
- `backend/crates/platform/auth/src/webauthn.rs`: add a bound verification helper that compares persisted binding and expected binding before or while consuming the ceremony.
- `backend/crates/workorder/rest/src/lib.rs`: add/use the mobile approval route and verify approval `step_up` before `approve_work_order`.
- `backend/app/src/collaboration.rs`: thread `PasskeyService` into `CollaborationState`, add/use the mobile poll-vote route, and verify poll-vote `step_up` before `with_audit` performs the vote mutation.
- `backend/app/src/lib.rs`: compose/pass the existing passkey service into the collaboration route.

Generated-client path:

- Change OpenAPI/schema/server first.
- Regenerate: `npm run gen:api:portable` and `npm run gen:api:swift`.
- Compile/check: `npm run check:ts`, `npm run check:kotlin`, `npm run check:swift`, and `npm run check:openapi-app`.
- Do not hand-edit generated clients.

## Regression coverage required downstream

Backend/API:

- start route persists binding and returns the same binding with a WebAuthn challenge.
- approval rejects missing, expired, consumed, wrong-user, and binding-mismatched step-up before mutation.
- poll vote rejects the same negative cases before vote/audit writes.
- a valid bound assertion consumes exactly once and allows exactly one mutation.

Android:

- approval and poll vote call the step-up service and pass the generated envelope to the gateway.
- null/cancelled credential leaves action waiting and does not call the API.
- replay creates one fresh envelope per queued item and attempt; one assertion cannot submit two queued actions.

iOS:

- same parity as Android.
- behavior tests cover challenge JSON conversion for wrapped `publicKey.challenge`.

Lifecycle gate:

- `node scripts/check-g007-collaboration-mobile-lifecycle.mjs` should remain green after code changes, then the final test card should run the focused Android/iOS checks from issue #407.

## Scope note

This contract chooses the mobile-scoped generated endpoints for initial enforcement. The existing web approvals page still calls `POST /api/work-orders/{workOrderId}/approve` without mobile step-up, and the existing web poll route may keep its current shape until web passkey UX is intentionally migrated. Do not claim mobile step-up coverage unless Android/iOS call only the mobile-scoped generated endpoints and cannot complete the same sensitive mobile approval or poll mutation through a null-assertion fallback path. If a future lane migrates web and mobile back onto shared endpoints, update this document and all affected callers in that same lane.
