---
name: consistency-code
description: Preserve consistency with the existing codebase when writing, modifying, or reviewing code. Match established naming, structure, patterns, abstractions, APIs, error handling, typing, imports, formatting, and architectural conventions. Prefer the project's existing way of doing something over introducing a new style, even when another approach might be equally valid. Use whenever implementing features, fixing bugs, refactoring, generating files, or reviewing code.
---

# Consistency Code

Code should look like it belongs in the codebase where it is written.

Consistency is more valuable than introducing a locally "better" pattern that makes the project less uniform. Before writing code, inspect nearby and analogous code and follow the conventions already established.

## Core rule

**Existing project conventions are the default source of truth.**

When multiple valid implementations exist, prefer the one already used by the codebase.

Do not introduce a new pattern merely because it is cleaner, newer, more elegant, or personally preferred.

If the existing convention is clearly harmful, incorrect, insecure, or causing the problem being solved, do not blindly reproduce it. Explain the inconsistency and use the smallest justified deviation.

## Inspect before writing

Before implementing something, inspect enough surrounding code to determine how the project normally handles the same kind of concern.

Look first for the closest analogous implementation rather than inventing one.

Pay attention to:

* file and folder organization
* naming conventions
* function and method structure
* class and interface patterns
* dependency injection
* imports and exports
* DTOs, schemas, entities, and types
* error handling
* validation
* logging
* async patterns
* return values
* null and undefined handling
* constants and enums
* configuration
* tests
* comments and documentation
* API response shapes
* persistence patterns
* framework-specific conventions

The closer the example is to the current feature or module, the more weight it should have.

## Match local conventions

Prefer consistency at the narrowest relevant scope.

Use this precedence when conventions differ:

1. The same file.
2. The same feature or module.
3. Similar features elsewhere in the project.
4. Project-wide conventions.
5. Framework or language conventions.
6. General best practices.

A project may intentionally use different conventions in different layers. Do not normalize those differences unless the task explicitly asks for it.

## Naming

Reuse the vocabulary already established by the domain.

If the project calls something `customerId`, do not introduce `clientId` for the same concept.

If analogous methods follow patterns such as:

`findOne`
`findById`
`create`
`update`
`remove`

follow those patterns instead of inventing alternatives.

Apply the same principle to variables, DTOs, events, routes, database fields, interfaces, enums, constants, and files.

Avoid introducing synonyms for concepts that already have established names.

## Structure

When adding a new implementation, compare it with similar existing implementations.

Match:

* responsibility boundaries
* method size and decomposition
* constructor patterns
* dependency placement
* public/private method organization
* file organization
* module composition
* declaration order

Do not extract helpers, services, abstractions, factories, utilities, or base classes unless similar abstraction is already used or the duplication clearly justifies it.

Do not inline established abstractions merely because the implementation would be shorter.

## Patterns and architecture

Respect the architectural direction already chosen by the project.

If a codebase consistently uses a repository, service, mapper, adapter, event, command, factory, interceptor, middleware, or another established pattern for a concern, use it for equivalent concerns.

Do not introduce an alternative architecture alongside an existing one without a concrete reason.

Avoid creating two ways of solving the same problem.

When extending a pattern, copy its **design principles**, not its code mechanically.

## API and contract consistency

New endpoints, functions, events, DTOs, messages, and public interfaces should resemble existing equivalents.

Preserve established conventions for:

* naming
* input shape
* output shape
* status codes
* errors
* pagination
* filtering
* optional fields
* serialization
* validation
* versioning

Do not silently create a special-case contract when an established contract already exists.

Preserve backward compatibility unless changing the contract is explicitly part of the task.

## Error handling

Use the project's existing error-handling strategy.

Do not mix approaches unnecessarily, such as returning `null` in one equivalent path, throwing exceptions in another, and returning result objects in a third.

Match existing exception types, error structures, logging behavior, and propagation rules.

Do not catch errors merely to rethrow them unless that pattern serves an established purpose.

## Types

Match the strictness and type patterns of surrounding code.

Reuse existing types when they represent the same domain concept.

Do not create nearly identical interfaces or types with different names without justification.

Avoid weakening types with `any`, broad casts, or unnecessary optional properties just to make an implementation compile.

If existing code distinguishes between persistence models, domain objects, DTOs, and API responses, preserve those boundaries.

## Imports and dependencies

Follow existing import conventions, including aliases, relative paths, barrel exports, ordering, and dependency boundaries.

Before adding a dependency, check whether the project already has a utility or dependency that solves the problem.

Do not add a new library for something already handled adequately by the current stack.

## Repetition versus novelty

Small amounts of repetition are often preferable to introducing a new abstraction that the rest of the project does not use.

Do not optimize for theoretical reuse.

First make the new code consistent with existing code. Extract shared behavior only when there is enough evidence that the abstraction belongs in the project.

## Refactoring

When refactoring, preserve existing conventions unless changing those conventions is the purpose of the refactor.

Do not mix unrelated stylistic cleanup into a focused change.

Avoid turning a bug fix or small feature into a broad consistency migration.

If nearby inconsistent code must be touched to implement the change safely, normalize only the smallest relevant area.

## Tests

Match the project's existing testing style.

Reuse established conventions for:

* test file location
* test naming
* setup
* fixtures
* mocks
* factories
* assertions
* integration versus unit boundaries

When adding tests for an existing component, make them look like neighboring tests rather than introducing a new testing philosophy.

## Comments

Match the project's commenting style.

Do not add comments that merely translate code into prose.

Prefer comments that explain constraints, non-obvious decisions, compatibility requirements, or why an unusual implementation exists.

Do not introduce large documentation blocks into otherwise self-explanatory code unless the codebase consistently uses them.

## Detecting inconsistency

While working, notice when the requested implementation would create a new convention.

Before doing so, ask:

**"Does the project already solve an equivalent problem another way?"**

If yes, prefer that approach.

If multiple competing conventions already exist, choose the one:

* used by the closest analogous code
* used by the newer active implementation
* used most consistently in the relevant module
* that creates the least additional variation

Do not attempt a project-wide cleanup unless requested.

## When deviation is justified

Consistency does not mean reproducing mistakes.

Deviation is justified when the established approach is:

* incorrect
* insecure
* deprecated
* incompatible with the requirement
* responsible for the bug being fixed
* significantly harmful to maintainability
* contradicted by an explicit project rule

When deviating, keep the difference intentional and localized.

State why the existing pattern should not be followed if the reason is not obvious.

## Final consistency check

Before finishing a code change, compare the new code against the closest existing equivalents.

Ask:

**"Would another developer familiar with this project think this code was written as part of the same system?"**

Check specifically for unnecessary differences in naming, structure, abstractions, contracts, typing, errors, imports, and tests.

If a difference has no meaningful reason, remove it.

## Principle

Do not make the codebase adapt to your preferred style.

**Adapt your implementation to the codebase.**
