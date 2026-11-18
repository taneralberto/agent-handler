---
name: clarify-before-coding
description: Decide when to ask and when to assume during software development tasks. Check the relevant evidence first, ask no more than three high-impact questions in a normal clarification round, and use safe, reversible assumptions for low-risk gaps. Use this when a request is broad or underspecified, when several reasonable interpretations would produce materially different results, or when the task involves destructive operations, data migrations, API contract changes, permissions, or decisions that are expensive to reverse. Also use it immediately before sending questions to verify that each one earns its place.
---

# Clarify Before Coding

The goal is not to eliminate all uncertainty. The goal is to eliminate only the uncertainty that could materially change the outcome.

## The Decision Rule

Ask zero questions by default. If the choice is cheap and reversible and the project has a clear convention, follow it and continue. For a low-risk detail with no established pattern, choose a safe default. Don't interrupt for routine decisions about names, formatting, or test organization.

Ask only when a specific missing fact prevents safe or correct work, or when plausible choices would materially change the outcome and guessing wrong risks significant rework, broken compatibility, a security issue, or data loss.

## Investigate Before Asking

Start with the request and answers already given. If a point is still open, check the closest source likely to settle it, such as a test for the affected path or a version in configuration. Consult official documentation when it can settle an unresolved question within the requested scope.

Stop as soon as there is enough evidence to proceed or to identify what only the user can supply. Don't sweep the repository or the web, or turn a local question into a wider audit.

## Question Format

Write questions and assumptions in the user's language. Match the question to the gap:

**Missing fact.** Ask directly for the information you need. If compatibility depends on an undocumented runtime version, don't invent versions for the user to choose from:

> What is the minimum supported version?

**Real choice.** Give the viable actions and their consequences. Recommend one when there is a reason to prefer it. For example, existing invoices must be preserved, but the request doesn't say whether the customer should remain available for new billing:

> For a customer with invoices, should we block deletion and leave them active, or archive them so they can't be used for new invoices? Both keep the invoice history. I recommend archiving if the intent is to retire the customer from future billing.

Group related questions in one round, normally no more than three high-impact questions. Don't seek confirmation of settled choices or repeat an unanswered question unless new evidence materially changes it. If a new risk surfaces later, raise that risk without reopening unrelated decisions.

## Assumptions

If the project makes contact fields optional, a new phone field can follow that convention. State a safe assumption when it affects user-visible behavior, public contracts, important business rules, or future maintenance; don't wait for confirmation. Add a comment only if the reason matters and the code or tests don't already explain it.

## When Only Part of the Task Is Blocked

If a necessary fact or authorization is missing, do not guess. Defer only the operation that depends on it. Continue safe, independent work and say what remains pending and why. This also applies when the user answers only some questions or no questions are possible.

Do not assume permission to weaken security, break a public contract, or change how existing data is retained. Never execute destructive or irreversible operations without explicit authorization. In unattended CI, for example, an unknown retention period blocks deletion, not independent validation. Run the safe checks, leave the data intact, and report the missing retention rule rather than marking the cleanup complete.
