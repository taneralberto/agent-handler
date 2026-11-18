---
name: aider-repo-map
description: Navigate unfamiliar repositories and trace behavior across modules when the entry point or dependencies are unclear. Use Aider's local repo map for broad discovery and follow concrete leads with targeted search and source reads.
---

# Navigate with Aider's repo map

Choose the first lookup from the evidence already available:

- Known path or stack frame: open the relevant lines and their enclosing function.
- Known symbol, route, event topic, error, or likely directory: search for that clue. For an exact string, use `rg -n -F -- 'literal' <directory>`. Follow the matches into source.
- Broad task with unclear entry points: consult an existing project guide or a map already in context. Generate a map if the relevant modules are still unclear.

## Generate a map

Generate a map only when repository-wide discovery is in scope; for a task limited to a directory, search within that directory instead.

Run from the relevant Git root (`git rev-parse --show-toplevel`). In a multi-repository workspace, start with the repository that owns the behavior.

```bash
aider --model gpt-4o --show-repo-map --map-tokens 2048 --map-multiplier-no-files 1 --no-gitignore --no-show-model-warnings --no-analytics --no-check-update
```

The model name supplies CLI settings. Map generation runs locally without an inference request. The explicit multiplier keeps the suggested budget predictable when invoked without file arguments.

Use 2048 tokens for initial orientation. Use 4096 directly for a repository-wide architecture survey, or expand once to 4096 if you still need to identify several modules. As soon as you have a plausible entry point, open it. If one specific module is missing, search its vocabulary, directory names, or contracts. Aider ranks symbols without seeing the OpenCode task.

## Follow and verify

Use map paths and signatures as leads. The `⋮` marker denotes omitted code. Aider's read-only preamble refers to its interactive chat. Follow the current task's permissions.

Trace only as far as the question or change requires. For example, to explain duplicate Telegram updates, start from the consumer, follow the decision that accepts or skips an update, and locate the cursor write or test that confirms the behavior. An entity declaration alone establishes the stored fields.

Before editing, read the implementation around the change and check the callers, contracts, or tests that depend on the behavior being changed. For HTTP or messaging boundaries, search both sides using the route or topic. Ground conclusions in the files actually inspected.

Reuse the map during exploration. Refresh it when a branch switch or structural change makes its paths stale. If Aider is unavailable or the map is empty, continue with file listing, targeted search, and direct reads. Keep generated maps in command output unless the user asks to save one.
