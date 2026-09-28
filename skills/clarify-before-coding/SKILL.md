---
name: clarify-before-coding
description: Decide when to ask and when to assume during software development tasks. Investigate first, ask no more than three high-impact questions in a normal clarification round, and proceed with explicit, safe, and reversible assumptions for everything else. Use this when a request is broad or underspecified, when several reasonable interpretations would produce materially different results, or when the task involves destructive operations, data migrations, API contract changes, permissions, or decisions that are expensive to reverse. Also use it immediately before sending questions to verify that each one earns its place.
---

# Clarify Before Coding

Asking too much hands the user back the work they delegated and breaks their flow. Asking too little produces rework, broken compatibility, security issues, or data loss.

The goal is not to eliminate all uncertainty. The goal is to eliminate only the uncertainty that could materially change the outcome.

## The Decision Rule

Before sending a question, identify at least one of the following:

* The materially different implementations that the possible answers would produce.
* The concrete missing fact without which the task cannot be completed safely or correctly.
If you cannot identify either one, do not ask. Follow the existing project convention, or choose the conventional, reversible option, and continue.

When different answers would produce different implementations, decide whether to ask based on the cost of being wrong:

* Cheap and reversible: assume and proceed.
* Significant rework, broken compatibility, data loss, security risk, or an irreversible decision: ask.
## Investigate Before Asking

Never ask for something you can discover yourself. Investigate in this order:

1. Reread the complete request and the rest of the conversation. The user may have already answered the question in an earlier message.
2. Inspect related files, types, tests, configuration, schemas, and project documentation.
3. Look for similar implementations in the project and reuse their conventions.
4. Check official documentation, API schemas, tool definitions, or library types when the uncertainty concerns an external framework, library, service, API, or command.
5. Separate what can already be inferred with reasonable confidence from what is genuinely still unresolved.
Do not ask the user for information that is already available in the repository, the conversation, the documentation, or accessible tooling.

## What Earns a Question

Only ask about ambiguities that materially affect expected behavior, scope, architecture, public contracts, data, security, compatibility, or the definition of done. In practice:

* **Behavior** — what the feature does on errors, edge cases, business rules, and excluded cases. → *Should suspended users still appear in historical reports, or should they be excluded?*
* **Scope** — several reasonable interpretations producing meaningfully different amounts or kinds of work. → *Does this apply only to the create form, or to the edit form as well?*
* **Data and migrations** — schema, integrity, existing records, historical information, retention, deletion. → *Should existing values be migrated to the new format, or remain in the previous format?*
* **External contracts** — endpoints, DTOs, events, messages, status codes, public types, published packages, third-party integrations. → *Should `customerId` remain temporarily supported for backward compatibility, or may this release break existing consumers?*
* **Security and permissions** — who may perform an action, who may view data, what is sensitive, where the authorization boundary belongs, whether an action requires an audit trail. → *Can an administrator view this information for all users, or only for users in their own organization?*
* **Destructive operations** — permanently deleting data, overwriting configuration, rotating or replacing secrets, rewriting history, removing compatibility, irreversible migrations, production changes that cannot be safely rolled back.
* **Expensive-to-reverse decisions** — architecture, dependencies, execution models, and other choices that would be costly to replace. → *Must this process finish within the same HTTP request, or may it run asynchronously through a queue?*
* **Missing external facts** — a deployment target, a minimum supported version, a resource identifier, a business deadline, an undocumented policy, which environment to modify. Ask plainly for the fact. Do not artificially convert a missing fact into a binary design choice.
## What Does Not Earn a Question

Do not ask when:

* The answer already exists in the code, types, tests, configuration, conversation, or documentation.
* The project already has a clear pattern.
* A standard convention exists and the choice is easy to reverse.
* The task is small, local, and low-risk.
* A safe default preserves future options.
* The answer would not materially change the implementation.
Do not hand the user decisions that belong to the agent professionally, including:

* Internal variable and function names.
* Basic function organization.
* Typing.
* Formatting.
* Standard error handling.
* Normal separation of concerns.
* Following established project style.
* Choosing between equivalent low-impact patterns.
* Applying standard maintainability practices.
Avoid these questions outright:

* "Do you want me to do it?"
* "Should I continue?"
* "What language are we using?" when the project already shows it.
* "Where should I create the file?" when the project structure makes it clear.
* "Do you want best practices?"
* "Do you want error handling?"
* "Should I add types?"
* "Do you prefer a simple or complex solution?"
* "Should I follow the existing style?"
* "Do you want tests?" when the repository already establishes a testing policy.
## Question Budget

Ask zero to three questions by default.

* Zero questions is the normal case for clear, local, and reversible tasks.
* Ask one question when there is one decisive ambiguity.
* Ask two or three questions when several independent decisions materially affect the solution.
* Exceed three only when the task is strategic, requirements-heavy, and cannot be implemented correctly without additional information.
The question budget is not a target. Do not invent questions merely because fewer than three have been asked.

Group related questions into one clarification round whenever possible.

## Timing

Ask at the beginning of the task, or immediately after the initial investigation, whenever the ambiguity could reasonably have been anticipated.

Do not interrupt implementation for minor preferences or details that convention can settle.

If a previously hidden constraint surfaces during implementation and creates a material risk:

1. Continue with all unaffected work.
2. Stop only the affected part.
3. Explain briefly what was discovered.
4. Ask one focused question about the newly discovered decision.
Raise a late discovery even when better investigation should have caught it earlier. Ask once, and note that it surfaced late. Staying silent about a real risk to preserve a single clarification round is the worse failure. What to avoid is a second round about preferences or details that were never blocking.

## Priority

When several questions are necessary, order them by impact:

1. Goal and expected behavior.
2. Scope.
3. Security and permissions.
4. Data and migrations.
5. Compatibility and external contracts.
6. Acceptance criteria.
7. Minor implementation preferences.
Minor preferences should normally be resolved using the conventions already present in the project.

## When the User Answers Only Some Questions

Proceed with safe and reversible assumptions for unanswered questions when possible.

Do not assume answers involving:

* Destructive operations.
* Security boundaries.
* Irreversible migrations.
* Permanent data loss.
* Breaking public contracts.
* Production changes that cannot be safely rolled back.
When an unanswered question affects only one part of the task, defer that part and complete the rest.

Do not ask the same question again later in the session unless new information materially changes its meaning.

## Question Format

Make answering inexpensive.

Use this structure when a decision is needed but other work can continue:

```text
Proceeding with: <work that can safely continue>

Decision needed for: <the affected part, in one sentence>
  a) <option> — <consequence>
  b) <option> — <consequence>
  Recommendation: <option> because <short reason>.

Assumptions: <safe and reversible assumptions currently being used>
```

Use this structure only when no safe progress is possible for the affected operation:

```text
Cannot safely proceed with: <specific affected operation>

Decision needed:
  a) <option> — <consequence>
  b) <option> — <consequence>
  Recommendation: <option> because <short reason>.
```

Do not describe the entire task as blocked when only one part is unresolved.

Include a recommendation whenever one option is clearly better. Do not force the user to design the solution from scratch.

Do not bias the decision by hiding the actual cost, risk, or limitation of the option you do not recommend.

Write questions and assumptions in the same language the user is using, even though this skill is written in English.

## Assumptions

When an ambiguity is minor, continue using an explicit, reasonable assumption.

A good assumption is:

* Conventional.
* Consistent with the existing code.
* Easy to change.
* Safe.
* Reversible.
* Unlikely to cause data loss.
* Unlikely to break compatibility.
Communicate assumptions when they affect user-visible behavior, public contracts, important business rules, or future maintenance.

Record an assumption in code only when it affects non-obvious behavior and cannot be expressed more clearly through the implementation itself. Do not add a comment that merely restates what types, naming, configuration, tests, structure, or existing documentation already say.

Unnecessary:

```ts
// Assumption: phone is optional
phone?: string;
```

The type already communicates the decision.

Useful:

```ts
// Assumption: legacy records without a timezone are interpreted as UTC
// to preserve the behavior of reports generated before migration 024.
```

## Reversible Decisions

When a decision is uncertain, prefer an implementation that is easy to change later:

* Wrap a business rule in a small function.
* Use configuration instead of hardcoded behavior.
* Add a new field as optional before making it required.
* Keep the previous contract working during a transition.
* Avoid destructive migrations.
* Isolate uncertain behavior behind a small interface.
* Make unsafe operations fail closed rather than guessing.
A reversible implementation is not permission to produce an invalid or knowingly incorrect solution.

Prefer the most reversible and least destructive valid path. If no safe valid path exists, defer the affected operation instead of inventing one.

## Tests

Follow the repository's established testing pattern for the behavior being changed. Do not introduce a new testing framework or restructure test infrastructure unless the task requires it.

## When Questions Are Not Possible

Questions may not be possible during autonomous runs, background jobs, CI execution, other non-interactive agent runs, or tasks where the user explicitly asked not to be interrupted. In those cases:

* Prefer the most reversible and least destructive valid path.
* Do not execute irreversible or destructive operations without explicit authorization.
* Prepare destructive changes without applying them when possible.
* Defer the affected operation if no safe implementation exists.
* Record important unresolved decisions in the final summary.
* Leave a localized marker in the code only when the repository permits such markers and the incomplete behavior cannot be mistaken for a finished implementation.
Do not leave a `TODO` that:

* Silently weakens security.
* Enables incomplete authorization.
* Executes destructive behavior.
* Pretends that an unimplemented feature works.
* Allows invalid data to be accepted.
* Can be deployed without an obvious failure.
When a code marker is inappropriate, keep the operation unimplemented, fail safely, and explain the pending decision in the final summary.

Having no one to ask does not license guessing on destructive operations. It licenses deferring them.

## Do Not Block Everything Over a Partial Doubt

When ambiguity affects only one part of the task:

1. Implement the parts that are safe and understood.
2. Isolate the uncertain behavior behind a simple boundary.
3. Avoid committing to an irreversible choice.
4. Ask only about the affected decision.
5. Clearly distinguish completed work from deferred work.
Delivering a safe partial implementation with one open decision is better than delivering nothing.

## Examples

### Clear Task — Do Not Ask

Request:

> Add validation to prevent negative quantities in the form.

Correct behavior:

* Inspect the form.
* Find the relevant control.
* Apply validation using the existing project pattern.
* Add or update tests if the repository has an established pattern for this behavior.
Do not ask which validator to use.

### Minor Ambiguity — Assume and Continue

Request:

> Add a phone field to the profile.

The project already treats contact information as optional.

Proceed with:

> I'll assume the phone field is optional, matching the other contact fields.

Do not interrupt the user for confirmation.

### Critical Ambiguity — Ask

Request:

> Let users delete customers.

Invoices reference customers, and hard deletion could destroy accounting history.

Ask:

> When a customer has invoices, should deletion be blocked or handled as a soft delete? I recommend a soft delete to preserve historical records.

### Hidden Risk Discovered During Implementation

Request:

> Remove inactive organizations.

During implementation, active invoices are found referencing inactive organizations.

Correct behavior:

* Continue implementing the unrelated filtering and validation.
* Stop the deletion portion.
* Explain the discovered relationship.
* Ask whether the organizations should be archived, excluded from new operations, or permanently deleted after their records are migrated.
Do not guess and delete the referenced data.

### Autonomous Run

Task:

> Apply the pending database cleanup during CI.

The cleanup includes an irreversible deletion whose retention period is undocumented.

Correct behavior:

* Do not execute the deletion.
* Prepare or validate the cleanup command if useful.
* Make the CI step fail safely or skip the destructive portion.
* Report the missing retention decision clearly.
## Final Rule

Investigate before asking. Ask less, but ask better.

Do not invent high-impact decisions, and do not delegate low-impact ones.

When no safe answer is available, defer the destructive part rather than guessing.