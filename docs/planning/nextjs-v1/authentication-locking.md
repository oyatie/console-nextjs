# Coordinated authentication Account lock transition

Dependency of accepted authentication ownership design `1d4d963306e4625404ccb8927e99468afff08aad82904627794937fba51fe2dd`. Base backend candidate `263d1106dba6185f198b78591d5260fb418444fa06f3d4c91f55955e4b894182`. Root is the sole writer. Mechanics independently reviewed; complete exact tests and implementation are not yet approved. Full R4 remains the objective.

Use native `FOR NO KEY UPDATE` on the exact Account and home Company as the existing-row mutex. It conflicts with lifecycle UPDATE/DELETE and other Account mutexes while permitting child/audit FK KEY SHARE. It does not confer authority, identify a natural person, or close proof-to-family issuance gaps. Protected Account generation still requires its own state and lock in the complete ownership transition.

Take Account locks before any bound ceremony/bootstrap/handoff/key/family/token lock; preserve a stable order within multirow sets. Unbound preauthentication start does not fabricate an Account. An unlocked locator is only a hint: recheck the exact Account/Company, current active state where required and original locator identity after the wait. If a target disappears or changes, fail this operation; never acquire a different Account after holding child locks. Expiration uses the current DB clock after all lock waits. Cleanup/logout may lock inactive Accounts; new authentication and issuance cannot use inactive Accounts.

## Complete coordinated scope

1. Shared WebAuthn registration and all three assertion finish paths, including borrowing transaction semantics. Registration must recheck its preliminary ceremony user before standard signature verification. Keep actual signed UV/binding/handle/counter behavior, final admission instant and atomic audit/rollback.
2. Bootstrap/reset/self-handoff and provisioning producers: Account lock before the first expired-bootstrap UPDATE or key DELETE. Bind exact Company. The inherited ForceReset/title-only recovery outcome is not accepted recovery and must be replaced in the full owner transition; these locking tests do not authorize exposure or certify it.
3. Both self-key deletion aliases: converge count/ownership/delete/audit in the auth owner, serialized with enrollment/deactivation. Last-key floor and actual actor evidence must survive concurrent deletion. Exact removal-purpose step-up remains required by the full design.
4. Refresh family issuance, rotation and logout: lock the resolved exact Account before bound family/token mutations and reread locator after waits. Preserve expiry caps/replay/reuse handling and inactive cleanup. No nested new pool transaction while holding Account on another connection.
5. Deactivation, including already-inactive retry, takes Account mutex before any sweep. No lifecycle generation/receipt guarantee is claimed until its owner migration lands.
6. Bound device/enrollment handoff creation, actual-proof approval, session alias approval and poll consumption: Account before bound mutation, exact target/approved identity and expiry rechecked. Reuse transaction-taking owners where a single operation already owns custody; avoid Account-lock self-deadlock across pool connections. The present separated proof/approval and poll-consume/family commits remain unaccepted gaps unless deliberately closed with exact receipt/admission proof. Unbound start stays independent.
7. Ordinary and forced Company removal: additive migration redefines the current owner functions to lock all target Accounts in sorted UUID order (including inactive ones) before any auth deletion or the dynamic child sweep. Preserve existing guards/ACLs and all historical bytes. This is not acceptance of inherited Company hard-deletion policy; former-worker/history preservation remains R4 work.
8. Roster: freeze phone-to-existing-Account mappings, lock complete existing set sorted by UUID before any row writes, then recheck all mappings. Disappearance/rename/intervening existing mapping is an explicit conflict. Process truly absent new phones in stable order; uniqueness races roll back the whole import. No late lookup/ON CONFLICT reclassification. The migration0036 cold-admin rehome is historical one-time DO, not a serving routine; never edit it.

## Required proof

Actual PostgreSQL observed waits, real owner operations and both lock schedules must demonstrate:

- Account wait precedes proof/key locks; Account KEY SHARE permits valid registration/authentication. Expired, inactive, changed or vanished locators cannot consume proof or create authority.
- Both self-delete aliases racing two keys preserve one key and exactly one deletion/audit.
- Reset/bootstrap, inactive-repeat cleanup, refresh issue/rotate/logout versus authentication do not deadlock; expiry after Account waits remains effective.
- Ordinary/forced removal with two Accounts and roster/auth traffic cannot lock children first, leak partial deletion or accept a removed Account.
- Bound handoff mutation/consumption versus deactivation/removal rechecks identity/expiry and cannot self-deadlock via nested pool transactions.
- Reversed/overlapping roster inputs and new-phone races have deterministic locking or visible whole-transaction conflict, without partial updates or duplicate bootstrap issuance.
- Preserve all current39authentication integration tests, all662existing migration assertions, and prove any added migration on fresh/historical/installed-baseline lineages with real interruption/retry. New suffix compilation must not reuse SQLx test binaries that embedded the previous directory inventory.

Tests are being written first. No production change is authorized from the WebAuthn-only subset: the coordinated scope above must have exact independent test approval before implementation. No live operation, secret, commissioning, deployment or release is authorized by this record.

## Removal-only exclusive lock clarification

Independently accepted by `/root/migration_review` within the existing design, before tests/implementation. Current0080/0196 functions plain-read Company; do not introduce an early blocking Company lock. Freeze target Account IDs and original Company status. Lock the frozen Accounts in UUID order with NO KEY UPDATE; upgrade only these Accounts with FOR UPDATE NOWAIT, then the exact Company with FOR UPDATE NOWAIT. Under those locks recheck exact membership, existence and status before any child or dynamic sweep. Failed upgrade, disappeared/added Account, or changed status aborts the whole operation; no late Account adoption or retry inside a partly mutated transaction. This prevents child-FK cycles and new-Account admission during deletion without a global table mutex.

The force command's inherited receipt counts are captured before the inner removal guard. Their concurrent population consistency remains HOLD; Account locking does not establish full business-write or exact-count custody. Hard-deletion policy, high-risk authority and retained former-worker records remain unaccepted R4 work. These tests do not authorize live deletion.

Public OTP verification and borrowed consumption also take the exact Account mutex before bound proof inspection/mutation; current active state, exact preliminary locator and DB-clock expiration are rechecked after waits. They do not close the separately required proof-to-family admission gap.
