# CEDAR POLICY AND OPERATIONAL GOVERNANCE SPECIFICATION

## 1. Architectural Mandate & Philosophy

In modern enterprise operations—encompassing Human Resources, Payroll Execution, Treasury/Banking, and Field Operations—security and compliance cannot be treated as technical black boxes hidden inside developer scripts or database column flags.

The **Acme Group / Oyatie Console Policy Engine** adopts the **Cedar Policy Specification** (formalized by AWS and open-source standards) as its authoritative evaluation model, tailored for non-technical business operators:

1. **Deterministic Evaluation**: Default-deny semantics where `permit` statements grant explicit capability, and `forbid` statements unconditionally override any permit.
2. **Natural Person Binding for Separation of Duties (SoD)**:
   - Corporate fraud and compliance failures often occur when an actor switches technical user accounts (e.g. using a service account or secondary role) to approve their own submissions.
   - In Console, Cedar evaluations bind to the underlying **`PersonIdentity` (`principal.person`)**, not merely the `Account`.
   - Any rule enforcing independence strictly requires: `principal.person != resource.preparedByPerson`.
3. **Constrained Policy Builder**:
   - HR managers, payroll officers, and branch managers must **never** be required to write raw Cedar DSL code, boolean regex, or SQL queries.
   - Console exposes a typed, constrained visual builder:
     - **Principal Scope**: Specific User Account or Role Group.
     - **Capability Bundle**: Pre-defined semantic business actions (e.g. `Payroll::ExecuteRun`, `HR::TransferEmployee`, `Approval::FinalizeTier3`).
     - **Entity/Tenant Jurisdiction**: Authorized corporate entities (`ENT-01`, `ENT-02`, or Group-wide).
     - **Resource Scope**: Typed resource selectors (`PA-*`, `PR-*`, `AP-*`, `WO-*`).
     - **Temporal Validity**: Explicit effective date range (`validFrom` to `validTo`).
     - **Assurance Gate**: Mandatory FIDO2 / Passkey biometric verification requirement.
     - **Monetary Thresholds (DoA)**: Maximum transaction limit per approval tier.
4. **Transparent, Explainable Access Inspection**:
   - For every access check, Console provides an immediate, plain-language answer to: *"Why can or can't this user perform this task?"*
   - Displays the full evaluation trace: evaluated principal, natural person, matched permissions, blocking constraints, and actionable remediation steps.
5. **Pre-Commit Impact Previews**:
   - Modifying a policy or granting a role displays a pre-commit diff:
     - Who gains access?
     - Who loses access?
     - What sensitive data classifications (`민감`, `대외비`, `비밀`) are affected?
6. **Independence-Preserving Accountable Handoff**:
   - Temporary delegation during employee leave or absence must verify that the delegate does not violate separation of duties for pending approvals.
7. **Product-Level Operational Recovery**:
   - Self-service recovery tools for expired CMS banking certificates, broken CSV/Excel import batches, interrupted payroll runs, and orphaned approvals without requiring code changes.

---

## 2. Core Cedar Formalism & Console Mapping

### 2.1 Entity Model
- **`Principal`**: `Console::Account::"<accountId>"` with attributes:
  - `person`: `Console::Person::"<personId>"`
  - `roleGroup`: `String` (e.g. `"PayrollOfficer"`, `"HRManager"`, `"BranchHead"`)
  - `entityId`: `String` (e.g. `"ENT-01"`)
  - `mfaLevel`: `"password" | "passkey_fido2"`
- **`Action`**: `Console::Action::"<Namespace>::<Verb>"`
  - Examples: `Payroll::ApproveRun`, `Payroll::ExecuteFirmBanking`, `HR::ApproveTransfer`, `Approvals::SignDoATier3`
- **`Resource`**: `Console::Resource::"<Kind>::<Id>"` with attributes:
  - `kind`: `String` (`"PR"`, `"PA"`, `"AP"`, `"WO"`, `"AT"`)
  - `entityId`: `String` (owning legal entity)
  - `preparedByPerson`: `Console::Person::"<personId>"`
  - `monetaryValue`: `Number` (KRW)
  - `dataClassification`: `"일반" | "대외비" | "민감" | "비밀"`
- **`Context`**: Request-level context attributes:
  - `time`: ISO 8601 timestamp
  - `ip`: Client IP
  - `mfaVerified`: `Boolean`
  - `delegatedFromPerson`: Optional `Console::Person`

### 2.2 Canonical Policy Templates

```cedar
// 1. Separation of Duties: No person may approve their own submission
forbid (
  principal,
  action in [Console::Action::"Approvals::Sign", Console::Action::"Payroll::ApproveRun"],
  resource
)
when {
  principal.person == resource.preparedByPerson
};

// 2. Tenant Boundary Isolation
forbid (
  principal,
  action,
  resource
)
when {
  principal.entityId != resource.entityId && principal.entityId != "ENT-ALL"
};

// 3. High-Value / Tier-3 DoA Requires Passkey Assurance
forbid (
  principal,
  action in [Console::Action::"Approvals::SignTier3", Console::Action::"Payroll::ExecuteFirmBanking"],
  resource
)
unless {
  context.mfaVerified == true
};
```

---

## 3. Explainable Access Engine Output

When an operator queries the Access Inspector, the engine returns a structured diagnostic:

```typescript
interface AccessExplanationResult {
  decision: "PERMIT" | "FORBID";
  summary: string;
  evaluatedAt: string;
  principal: {
    accountId: string;
    username: string;
    personId: string;
    personName: string;
  };
  action: string;
  resource: {
    id: string;
    code: string;
    kind: string;
    entityName: string;
    preparedByName?: string;
  };
  matchedPermitRules: Array<{
    id: string;
    title: string;
    scope: string;
  }>;
  blockingForbidRules: Array<{
    id: string;
    title: string;
    violationReason: string;
  }>;
  remediationAdvice?: {
    canSelfRemediate: boolean;
    recommendedAction: string;
    actionType: "passkey_register" | "grant_request" | "handoff_delegate" | "switch_entity";
  };
}
```

---

## 4. Operational Recovery Engine Specifications

Console implements autonomous self-service resolution for 4 major enterprise failure modes:

| Failure Mode | Root Cause | Operator Self-Service Resolution |
| :--- | :--- | :--- |
| **Banking Certificate Expired** | KFTC CMS client certificate past 365-day validity | In-place Certificate Re-issuance & Key Exchange wizard with test ping. |
| **Batch Import Validation Failure** | Missing resident IDs or mismatched bank codes in uploaded file | Interactive staging grid with inline cell repairs and schema re-validation. |
| **Interrupted Payroll Run** | Network timeout during 100-byte CMS payload transmission | Safe idempotent rollback or sequence-resuming replay without double-crediting. |
| **Stalled Approval Deadlock** | Assigned approver terminated or incapacitated | Independent Audit Committee bypass or automated deputy escalation. |
