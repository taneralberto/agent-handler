---
name: kiss-for-you
description: Use this skill BEFORE implementing or extending a module or feature. Guide the agent to design KISS solutions from the start few edit points, clear names, one source of truth for repeatable cases, and zero unnecessary ceremony. Also use it to simplify existing code without changing behavior.
---

# Skill: preventive KISS implementation

Use this skill **before writing new code** or extending an existing module. The goal is not to refactor after the damage is done. The goal is to prevent the module from starting with too many resolver functions, closed types, redundant names, or scattered edit points.

The main question is always:

> If I add another similar case tomorrow, how many places will I need to touch?

The desired answer is:

```txt
One place to register the case + specific rendering only if the screen really changes.
```

If the new solution already requires `supportsX`, `isX`, `loadX`, `exportX`, a union type, and a manual button for every case, the design is already going in the wrong direction.

---

## When to use it

Use this skill **before**:

- adding a new case to an existing screen;
- creating a family of reports, filters, tabs, views, or exporters;
- adding a new endpoint with the same shape as existing ones;
- copying an `if` or `switch` block for another case;
- creating closed literal types for values that will keep growing;
- creating new `state`, `config`, `resolver`, `adapter`, or `mapper` files.

Also use it reactively if you already see signs like these:

- Many `resolve*`, `build*`, `map*`, or `config*` functions that only call other functions.
- Long type names that repeat the folder or feature name.
- Closed unions that force several files to change for every new case.
- One getter per case: `supportsX`, `isX`, `loadX`, `exportX`.
- Two nearly identical functions with small, implicit differences.
- Separate files that actually belong to the same concept.
- A simple change requires touching 5 or more places.

---

## Relationship with other skills

This skill does not replace `minimal-changes`, `respect-existing-codebase`, or `simple-clean-code`.

- Use `minimal-changes` to control scope and avoid opportunistic edits.
- Use `respect-existing-codebase` to preserve the repo's style, architecture, conventions, and domain language.
- Use this skill to prevent structures that are hard to extend.
- Use `simple-clean-code` for general code hygiene: readability, pragmatic typing, simple control flow, and clear names.

If there is a conflict:

- Scope and existing behavior win over cleanup.
- Project conventions win over personal preferences.
- KISS wins over copying local patterns that clearly create multiple edit points.

---

## Principles

| Principle           | Practical rule                                                                                              |
| ------------------- | ----------------------------------------------------------------------------------------------------------- |
| KISS                | Prefer direct code over elegant architecture.                                                               |
| Fewer edit points   | Optimize for reducing future edit points.                                                                   |
| One source of truth | If there are N cases, try to keep them in one flat list.                                                    |
| Short names         | Remove words that repeat obvious context.                                                                   |
| No fake generics    | Share load/export if the contract is the same; do not force generic rendering if the columns are different. |
| Stable behavior     | Do not change business rules while cleaning structure.                                                      |
| Inline when obvious | If a one-use function only builds a simple string or object, inline it.                                     |

---

## Policy for resolver functions

Do not create `resolve*`, `build*`, `map*`, `to*`, or `get*Config` functions by default.

Before creating one of these functions, the agent must justify why the code cannot stay inline.

### They are justified when

- They encapsulate real business logic.
- They are used in more than one place.
- They reduce a complex branch that would be hard to read inline.
- They isolate important validation.
- They transform data in a non-obvious way.
- They have their own tests or a clear contract.

### They are not justified when

- They only pass data from one function to another.
- They only build a flat object.
- They only rename fields.
- They only return strings.
- They have a single call site.
- They force the reader to jump across files to understand 5 lines.
- They exist only to make the code “look organized”.

### Example: avoid decorative resolvers

Do not start like this:

```ts
const config = resolveDerivedReportConfig(report);
const state = buildDerivedReportState(response, config);
this.derivedState = state;
```

If the transformation is simple, start like this:

```ts
const rows = response.report.rows;

this.derivedState = {
  report,
  rows,
  pages: paginateRows(rows, REPORT_PAGE_SIZE),
  printTitle: `${report.title}_${this.snapshotId}_${this.dateNow}`,
};
```

Rule:

```txt
If a resolver does not reduce real complexity, it creates accidental complexity.
```

---

## Work process

### 0. Design the change before writing code

Before creating files, types, or functions, answer:

```txt
Which part of this change is just another case in a list?
Which part really needs its own UI or logic?
```

Example:

```txt
New derived report:
- Another case in a list: button, backendType, documentType, load, export.
- Own UI: only if its columns cannot use the current renderer.
```

If you cannot answer that, do not implement yet.

---

### 1. Measure the cost of adding the next case

Before changing code, answer:

```txt
To add the next similar case, which files and symbols would need to change?
```

Make a concrete list:

```txt
- union type
- config record
- supportsX getter
- isX getter
- HTML button
- loadX
- exportX
- service shim
- renderer
```

If the list is long, do not start by creating branches. First design one source of truth.

---

### 2. Separate what is common from what is truly different

Not everything must be generic.

Correct example:

```txt
Load/export: generic, because all cases use snapshotId + backendType + documentType.
Render: specific, because each report may have different columns.
```

Rule:

```txt
Share the flow if the contract is the same.
Separate rendering if the screen is different.
```

---

### 3. Start with a flat list when there will be N cases

Do not start like this:

```ts
get supportsCreditNotes(): boolean {
  return this.available.includes('credit_notes_report');
}

get supportsDebitNotes(): boolean {
  return this.available.includes('debit_notes_report');
}

loadDerivedReport(): void {
  if (this.active === 'credit_notes') {
    return this.service.getCreditNotes(...);
  }

  return this.service.getDebitNotes(...);
}
```

This works for two cases, but it punishes the third.

Start like this:

```ts
const DERIVED_REPORTS = [
  { id: 'credit_notes', label: 'N/C', backendType: 'credit_notes_report', documentType: 'creditNote' },
  { id: 'debit_notes', label: 'N/D', backendType: 'debit_notes_report', documentType: 'debitNote' },
];

get derivedReports() {
  return DERIVED_REPORTS.filter((report) => this.available.includes(report.backendType));
}
```

Result:

```txt
Adding a report = adding one entry to the list.
```

---

### 4. Do not create per-case shims if the contract is the same

Do not start like this:

```ts
getCreditNotesReport(id, params) {
  return this.getDerivedReport(id, 'credit_notes_report', 'creditNote', params);
}

getDebitNotesReport(id, params) {
  return this.getDerivedReport(id, 'debit_notes_report', 'debitNote', params);
}
```

If a method only passes constants, it does not deserve to exist.

Start like this:

```ts
getDerivedReport(id: number, backendType: string, documentType: string, params: DateRange) {
  return this.http.get(`${this.apiUrl}/${id}/derived/${backendType}`, {
    params: { ...params, documentType },
  });
}
```

Rule:

```txt
If the shim only passes constants to a generic function, delete it or do not create it.
```

---

### 5. Use short names from the start

If the file is already inside `declared-report-detail`, do not repeat `Declared` in every name.

Before:

```ts
SalesBookDeclaredViewState;
AccountReportDeclaredViewState;
DeclaredSalesBookPrintState;
```

After:

```ts
SalesBookState;
AccountReportState;
SalesBookPrintState;
```

Rule:

```txt
If the name repeats the folder, feature, or class, shorten it.
```

---

### 6. Do not call something `Response` if it does not come from the backend

Before:

```ts
type SalesBookResponse = { kind: "invalid-shape"; message: string } | { kind: "ready"; state: SalesBookState };
```

Problem: it sounds like an HTTP response, but it is not.

After:

```ts
type SalesBookLoadResult = { kind: "invalid-shape"; message: string } | { kind: "ready"; state: SalesBookState };
```

Rule:

```txt
Use Response only for real backend responses or DTOs.
```

---

### 7. Inline one-use functions

Before:

```ts
export function buildPrintTitle(id: number | null, dateNow: string): string {
  return `Report_${id ?? 'snapshot'}_${dateNow}`;
}

get printTitle(): string {
  return buildPrintTitle(this.snapshotId, this.dateNow);
}
```

After:

```ts
get printTitle(): string {
  return `Report_${this.snapshotId ?? 'snapshot'}_${this.dateNow}`;
}
```

Rule:

```txt
If a function has one call site and only builds a simple string or object, inline it.
```

---

### 8. Unify duplicates with explicit differences

Before:

```ts
buildQueryFilters();
buildExportFilters();
```

Both do almost the same thing, but one includes defaults and the other does not.

After:

```ts
buildFilters({ includeDefaults: true });
buildFilters({ includeDefaults: false });
```

Rule:

```txt
Unify duplicates only if the difference is named in the parameter.
```

Do not do this:

```ts
buildFilters(true);
buildFilters(false);
```

Do this:

```ts
buildFilters({ includeDefaults: true });
```

---

### 9. Remove defensive branches that are not real flows

Before:

```ts
const adapter = getAdapter("sales_book");

if (!adapter) {
  return { kind: "unsupported" };
}
```

If the function is only called for `sales_book`, a missing adapter is an internal configuration bug, not a normal functional case.

After:

```ts
const adapter = getAdapter("sales_book");

if (!adapter) {
  return { kind: "invalid-shape", message: "Missing adapter for sales_book" };
}
```

Rule:

```txt
Do not model configuration bugs as normal application states.
```

---

## Checklist before implementing

- [ ] Do I know how many files the next similar case would touch?
- [ ] Do cases live in one flat list, or am I spreading them across getters and switches?
- [ ] Am I about to create `supportsX`, `isX`, `loadX`, or `exportX` per case?
- [ ] Am I about to create a union type that must be edited every time the domain grows?
- [ ] Am I about to create a function that only passes constants to another function?
- [ ] Does the rendering really change, or do only data values such as label/backendType/documentType change?
- [ ] Do names repeat the folder or feature?
- [ ] Am I calling something `Response` even though it does not come from the backend?
- [ ] Can I solve this with one simple source of truth?

---

## Checklist after implementing

- [ ] Adding the next case touches one list or one clear point, not 5 files.
- [ ] I did not add managers, factories, or unnecessary abstractions.
- [ ] I did not add shims that only pass constants.
- [ ] Names are short but still clear.
- [ ] Specific rendering exists only if the UI really changes.
- [ ] Existing behavior is preserved.
- [ ] No stale imports or symbols remain.
- [ ] Diagnostics/typecheck were run.
- [ ] HTML/templates were manually checked if string replacements were made.

---

## Anti-patterns to avoid

Do not replace one complexity with another.

Avoid this:

```ts
class DerivedReportManagerFactoryResolver {}
```

Avoid this:

```ts
type CreditOrDebitOrRetentionOrMunicipalOrFutureReport = ...
```

Avoid this:

```ts
if (report === "a") loadA();
if (report === "b") loadB();
if (report === "c") loadC();
```

Prefer this:

```ts
const REPORTS = [
  { id: "a", backendType: "a_report" },
  { id: "b", backendType: "b_report" },
  { id: "c", backendType: "c_report" },
];
```

---

## Expected agent response

When using this skill, the agent must respond with:

1. Preventive plan before writing code.
2. How many edit points the next case would require.
3. Proposed source of truth.
4. What will be generic and what will be specific.
5. Proposed short names.
6. What it will not create to avoid ceremony.
7. Risks.
8. Verification.

Example:

```txt
Preventive plan:
The new derived report will be another case in REPORTS[]. Load/export stay generic; rendering only separates if columns change.

Future edit points:
Adding another report should touch one REPORTS[] entry and, if needed, one render case.

I will not create:
supportsX, loadX, exportX, service shims, or union types per report.
```

---

## Final rule

Design first so the next case is easy.

If a new implementation already starts by forcing many edit points, it is not flexible. It is only arranged nicely.
