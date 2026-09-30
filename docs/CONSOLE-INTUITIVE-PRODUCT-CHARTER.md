# Console Intuitive Product Charter & Operational UX Standard

> **Authority**: Product Design Charter & Usability Mandate  
> **Target System**: Acme Group / Oyatie Console (`oyatie/console` & `frontend`)  
> **Core Principle**: Console becomes intuitive when people can recognize their work, understand the next action, and safely see the consequences before committing. Users should not need to understand Console’s database, permission system, or internal terminology to use it.

---

## The Operational Standard

The supporting technical foundations—persistent identities, authorization, revisions, and owning business operations—are necessary but are **not a usable experience by themselves**. The required product work is turning those foundations into **recognizable tasks, meaningful choices, and recoverable decisions**.

“Anyone can use it” has a practical interpretation:
- An **employee** should find submitting leave straightforward.
- A **payroll specialist** should find payroll preparation straightforward.
- Console must explain rules and prevent avoidable mistakes, but it **cannot remove the professional judgment** required to approve payroll.

---

## 1. An Obvious Place to Start, Based on What the Person Is Trying to Accomplish

The first useful screen must answer four fundamental questions:
1. **What needs my attention?**
2. **What am I currently working on?**
3. **What happens next?**
4. **Where did I leave off?**

### Requirements
- **Recognizable subjects and actions**: Present actual assigned work, pending approvals, upcoming deadlines, and saved drafts. Each item must state its business subject and action:
  - *Good*: `Review September payroll · Seoul Company`
  - *Unacceptable*: `3 pending objects` or `AP-3121 pending`
- **Genuine empty states**: For a new or empty workspace, show the genuine next setup task and why it matters:
  - *Good*: `Add your first organizational unit` or `Register an employment`
  - *Unacceptable*: Do not fill the screen with invented activity or artificial sample employees.
- **Interrupted work continuity**: Someone returning halfway through payroll preparation must be able to reopen the saved payroll run with all entered inputs, adjustments, and unresolved issues intact. They must not have to reconstruct how they reached it.

---

## 2. Navigation Organized Around Familiar Work

There must be a clear distinction between **people**, **employment**, and **organization**. Navigation is organized into five top-level functional groups:

| Destination | What a person should expect to find |
| :--- | :--- |
| **Work** | My tasks, approvals, deadlines, and interrupted work |
| **People** | People, employment, organization, attendance, and payroll |
| **Communications** | Messenger, mail, and calendar |
| **Data** | Datasets, analyses, sheets, documents, applications, and models |
| **Administration** | Access, integrations, configuration, and operations |

### Structural Rules
- **Direct visibility for payroll**: Payroll must remain directly visible within **People**. Approvals belong in **Work**, but every payroll record must also display its own approval progress in context.
- **Multiple sensible entry points**: Users must have multiple natural ways to reach the same authoritative record:
  - An employment can be reached through the **Person**, through their **Organizational Unit**, or through the **Employment List**.
  - These are different entry points into *one authoritative record*.
  - *Rationale*: People do not all think in the same order. One person starts with *“Who is this person?”*, while another starts with *“Who works in this team?”*. Both journeys must succeed effortlessly.

---

## 3. Every Screen Makes Its Subject and Scope Unmistakable

Before anyone changes anything, they must be able to answer four questions immediately:
1. **Which Company am I working in?**
2. **Which person, employment, team, or payroll period is this?**
3. **Is this current, proposed, or historical information?**
4. **Who is responsible for the next step?**

### Requirements
- **Local context display**: Company, period, effective date, and status must appear **where the decision happens**—not solely in a distant global top-bar selector.
- **Multi-company transparency**: When operating across multiple Companies within a Group, a Group view can summarize collective progress, but **each payroll or HR action must visibly identify its legal employer and Company**.
- **Business language over internal IDs**: Internal UUIDs, catalog versions, and revision counters do not communicate human context. They belong in technical detail panels for auditors. The primary presentation must use **names, dates, relationships, and business status**.

---

## 4. The Product Teaches Where Information Belongs Through Real Relationships

A person, their Account, their employment, and their organizational assignment are closely related, but fundamentally distinct:

| Information | Where it belongs |
| :--- | :--- |
| **Login credentials and personal access** | **Account** |
| **The person’s identity** | **Person** |
| **Employer, employment dates, and employment terms** | **Employment** |
| **Department, team, position, and assignment dates** | **Organization and assignment** |
| **Pay inputs and calculated results for a period** | **Payroll** |
| **Request, reviewer, decision, and reason** | **Approval** |

### Interaction Model
- Users should not need to memorize this distinction; the interface must embody it.
- **Prefill known relationships; ask only for the missing decision**:
  - From a person's page, clicking *“Add employment”* must carry that person into the workflow automatically.
  - From a Company’s organization page, clicking *“Add department”* must already know the Company and the selected parent Site.
  - From payroll, opening a person’s employment must preserve the active payroll context and provide a single-click path back.
- *Rule*: Repeatedly asking users to re-enter information Console already knows creates friction, uncertainty, and data inconsistency.

---

## 5. Inputs Communicate What to Enter Before an Error Occurs

A plain text box with a label is inadequate for consequential enterprise data. Every input requires an appropriate combination of clear label, suitable control, example, unit, default, and explanation:

| Input | Appropriate Interaction |
| :--- | :--- |
| **Employee or approver** | Search and select an authorized person; distinguish matching names using permitted context (department, role, site). |
| **Organizational parent** | Choose from eligible existing units, with the hierarchy visibly rendered. |
| **Effective date** | A date control with a visible example and explanation of when the change takes effect. |
| **Payroll period** | A month or period selector, with exact start and end dates displayed. |
| **Money** | A numeric input with currency symbol, grouping separators, and permitted precision clearly shown. |
| **Working time** | Explicit hours/minutes or start/end times, with the applicable timezone explicitly stated. |
| **Employment category** | Named choices with short explanations where legal distinctions matter (e.g., 정규직, 계약직, 특수고용). |
| **Supporting document** | Accepted file formats and size limits shown *before* selection, followed by explicit upload status. |

### Elimination of Ambiguity
- **Dates**: Must never force users to guess whether `09/10` means September 10 or October 9 (use `2026년 9월 10일` or `2026-09-10`).
- **Amounts**: Must never leave users guessing whether the unit is won, thousands of won, or a decimal currency.
- **Object references**: Must be a selection of an actual existing object, never a free-text name or UUID field.
- **Defaults**: Defaults must be visible and defensible. Defaulting to the active Company is helpful; silently guessing a legally binding employment category is dangerous.

---

## 6. Validation Helps the Person Succeed, Without Punishing Normal Input

Console must accept harmless input variations when their meaning is unambiguous, and immediately show the normalized result (e.g., accepting currency grouping commas and displaying the exact parsed amount).

When meaning is ambiguous, Console must ask for clarification—it must **never silently convert ambiguous input into a consequential business fact**.

### Good Validation Principles
1. **Explain the specific problem beside the field**, not in a generic modal alert.
2. **Preserve everything already entered**; never reset the form on failure.
3. **Offer actionable corrections** when valid alternatives are known:
   - *Good*: `“This department belongs to another Company. Choose a department in Seoul Company.”`
   - *Unacceptable*: `“Invalid reference”` or `“400 Bad Request”`
4. **Check dependent rules before submission**, providing early warnings.
5. **Use the identical validation rules** during live preview and final execution.
6. **Respect Korean IME composition**, never rejecting or breaking unfinished Hangul syllables (`ㄱ` → `가` → `강`).
7. **Distinguish invalid from unusual**: An unusually large allowance warrants an advisory review warning; that does not make it invalid. Required blocking errors and advisory warnings must look and behave differently.

---

## 7. People Can Examine Consequences Before Making a Commitment

Anxiety in enterprise software arises from not knowing what a button will actually do.

### Consequence Preview in Business Language
Before any consequential action, display the exact proposed outcome in plain business language:

> **Example**:
> *“Transfer Kim to Operations, effective October 1.*  
> *The current assignment remains in history through September 30.*  
> *This request requires approval before taking effect.”*

- For **payroll submission**: Show the exact period, included population count, gross/net totals, unresolved discrepancies, and the assigned reviewer.
- For **access changes**: Show exactly who receives access, to what data tier, and until when.

### Consequence-Naming Button Labels
Button labels must explicitly name the consequence:
- *Good*: `Save draft`, `Submit for review`, `Approve payroll`, `Export payment instructions`
- *Unacceptable*: A generic `Confirm` or `OK`, which forces the user to guess what is being committed.

### Preserving Meaningful Business Distinctions
- **Downloading payment instructions is not paying wages.**
- **Submitting a request is not approving it.**
- **A recorded historical approval does not mean the record is still the current active version.**

---

## 8. Progress Is Preserved, and Recovery Is Understandable

Confidence requires knowing that an interruption or mistake will not destroy hours of work.

### Core Capabilities
- **Durable drafts & visible save state**: Clearly indicate `All changes saved to draft` or `Unsaved changes`.
- **Stable links**: Direct, shareable URLs to records and requests.
- **Reopening fidelity**: Resuming an interrupted workflow restores all entered inputs, adjustments, and attachments.
- **Concurrent change comparison**: Clear side-by-side comparison if another user modified the record in the interim.
- **Safe retries**: Idempotent, safe retry pathways after network interruptions.
- **Audit history**: Detailed, human-readable timeline of who changed what, when, and why.
- **Appropriate correction paths**:
  - *Before commitment*: Editing a draft is direct and simple.
  - *After payroll approval*: Correction requires an official new revision and approval workflow. The interface must guide users through that legitimate path rather than promising an “Undo” button it cannot legally or safely perform.
- *Storage truth*: “Saved” means durable server acknowledgment. If confirmation is lost, show that confirmation is unavailable and allow checking the original request without risking duplicate submissions.

---

## 9. Depth Appears When Needed, While the Immediate Task Remains Clear

A cohesive enterprise tool functions like a consistent, predictable world:
- **Persistent entities** that can be recognized and revisited.
- **Visible goals** on every screen.
- **Context-appropriate actions** that make sense in the current state.
- **Feedback** that explains what changed after every action.
- **Preserved progress** across sessions.
- **Progressive disclosure**: Deeper information is accessible directly from the object being inspected.
  - Selecting an organizational unit reveals its permitted relationships, headcount, and actions.
  - Selecting a payroll discrepancy reveals the underlying source, statutory explanation, and direct repair path.

### Coexistence of Roles
- **Power users / Specialists**: Require dense tables, comprehensive keyboard shortcuts, rapid filtering, and bulk operations.
- **Occasional users / Beginners**: Require self-explanatory labels, prefilled context, and guided prompts.
- Both coexist cleanly through progressive disclosure and sensible defaults, without hiding consequential outcomes.

---

## 10. Polish Supports Comprehension Rather Than Decorating an Complete Workflow

Typography, spacing, visual hierarchy, and alignment exist to help users understand:
- The **subject** from supporting detail.
- **Editable input** from calculated output.
- An **advisory warning** from a **blocking error**.
- A **draft** from a **committed result**.
- The **primary next action** from secondary options.

### Polish Criteria
- **Korean typography**: Korean text must remain readable with correct word breaking (`keep-all`) and line heights.
- **Numeric alignment**: Currency amounts and numeric quantities must use tabular, right-aligned figures.
- **Keyboard navigation**: Visible focus rings for keyboard-only navigation.
- **Information density**: Narrow screens must retain both inputs and evidence rather than truncating critical context.
- **Color accessibility**: Color must never be the sole signal of state or status (always combine color with text and icons).
- **Zero stubs**: A beautiful page with a dead button is broken. A technically correct page that sends users to raw JSON or demands internal UUIDs is equally incomplete.

---

## Concrete Acceptance Tests

A release is validated against the standard when an ordinary user can perform end-to-end tasks without technical instruction:

1. **Organization Setup Test**:
   - Create a new Site.
   - Add its Department and child Team.
   - Correct an accidental typo in a name.
   - Reopen the saved structure.
   - Explain what has changed from the revision history.
   - Repeat under restricted permissions and with a simulated network interruption.

2. **Payroll Preparation & Approval Test**:
   - Open and prepare a real payroll period.
   - Identify and repair a flagged statutory discrepancy (e.g., unapproved overtime or missing tax code).
   - Review the consequence summary.
   - Submit for review.
   - Have an authorized reviewer inspect, approve, and seal the run.
   - Reopen the exact reviewed result, verifying that inputs, calculated net pay, and audit trails remain identical.
