---
name: kiss-for-you
description: Use this skill before implementing or extending a module or feature. Prefer direct changes and clear names. Reduce edit points when working with an existing family of cases or a rule that is already scattered. Also use it to simplify existing code without changing behavior.
---

# Skill: preventive KISS implementation

Make the simplest correct change that meets the request. For a one-off backend validation bug, fix the affected rule and check the failing input. That does not require a list of variants, a UI design, or an abstraction for a hypothetical second case.

## When edit points matter

Only when the change involves an existing family of equivalent cases or the same rule is already spread across several places, ask yourself:

> If I add another similar case, how many places will I need to touch?

If neither applies, skip the question and proceed. Look for edits that repeat the same fact, such as adding the same identifier to a getter, a switch, and a service shim. Prefer one source of truth for that fact, not one file for everything.

### Separate what is common from what is truly different

For example, credit-note and debit-note reports already load and export using `snapshotId`, `backendType`, and `documentType`. Instead of switching on the report in both paths, keep the differing fields in the report definitions:

```ts
const reports: Record<ReportKind, ReportSpec> = {
  credit_notes: { backendType: 'credit_notes_report', documentType: 'creditNote' },
  debit_notes: { backendType: 'debit_notes_report', documentType: 'debitNote' },
};
```

Load and export read the same entry. If a report needs different columns, keep its renderer separate. Sharing a request contract does not mean sharing every behavior.

## Helpers and local logic

A single caller is not a reason to reject a function. A helper such as `validateDateRange` can isolate important validation or a business rule from request assembly. Inline simple delegation or trivial formatting when the expression is clearer at the call site.

- If query and export filters differ only in whether they include defaults, `buildFilters({ includeDefaults: true })` makes the difference explicit. Keep genuinely different rules separate.
- Use short names that describe the value: `SalesBookLoadResult` for a local result, not `SalesBookResponse`. Drop words that only repeat the surrounding context.
- Treat a missing adapter for a supported case as a configuration error, not an ordinary “unsupported” result.

## Scope and conventions

Follow the project's domain language and conventions (see `consistency-code`). Don't widen a closed type to `string`, drop an exhaustive check, or change a public contract just to touch fewer files. Preserve behavior outside the requested change.

Remove imports or symbols made obsolete by the change; leave unrelated cleanup alone. Verify the affected behavior with the project's usual checks.

Follow the task and your role's response format. Explain a trade-off when it helps the reader understand a decision; this skill requires no separate plan or report.
