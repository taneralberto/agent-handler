use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Supported OpenCode agent modes. Matches the local SDK evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub enum Mode {
    subagent,
    primary,
    all,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::subagent => "subagent",
            Mode::primary => "primary",
            Mode::all => "all",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "subagent" => Ok(Mode::subagent),
            "primary" => Ok(Mode::primary),
            "all" => Ok(Mode::all),
            other => bail!(
                "invalid mode `{}` (expected subagent, primary, or all)",
                other
            ),
        }
    }

    pub fn next(self) -> Self {
        match self {
            Mode::subagent => Mode::primary,
            Mode::primary => Mode::all,
            Mode::all => Mode::subagent,
        }
    }
}

/// Permission action vocabulary. Matches the OpenCode SDK `PermissionAction`
/// type: `"allow" | "deny" | "ask"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionAction {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "ask")]
    Ask,
    #[serde(rename = "deny")]
    Deny,
}

impl PermissionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionAction::Allow => "allow",
            PermissionAction::Ask => "ask",
            PermissionAction::Deny => "deny",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "allow" => Ok(PermissionAction::Allow),
            "ask" => Ok(PermissionAction::Ask),
            "deny" => Ok(PermissionAction::Deny),
            other => bail!(
                "invalid permission action `{}` (expected allow, ask, or deny)",
                other
            ),
        }
    }
}

/// The set of supported permission keys. Mirrors the local SDK evidence.
pub const PERMISSION_KEYS: &[&str] = &[
    "read",
    "edit",
    "glob",
    "grep",
    "list",
    "bash",
    "task",
    "external_directory",
    "todowrite",
    "question",
    "webfetch",
    "websearch",
    "lsp",
    "doom_loop",
    "skill",
];

/// A single agent definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub description: String,
    pub mode: Mode,
    pub model: Option<String>,
    pub prompt: String,
    pub permissions: BTreeMap<String, PermissionAction>,
}

impl Agent {
    pub fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() {
            bail!("agent name must not be empty");
        }
        if name.starts_with('-') || name.ends_with('-') {
            bail!("agent name `{}` must not start or end with `-`", name);
        }
        if name.contains("..") {
            bail!("agent name `{}` must not contain `..`", name);
        }
        let mut prev = '-';
        for ch in name.chars() {
            if ch == '-' {
                if prev == '-' {
                    bail!("agent name `{}` must not contain consecutive hyphens", name);
                }
                prev = ch;
                continue;
            }
            if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() {
                bail!(
                    "agent name `{}` may only contain lowercase letters, digits, and single hyphens",
                    name
                );
            }
            prev = ch;
        }
        Ok(())
    }

    /// Validate an optional model identifier. `None` (or blank) means inherit
    /// OpenCode's default and is always acceptable.
    pub fn validate_model_opt(value: &Option<String>) -> Result<()> {
        if let Some(v) = value {
            Self::validate_model(v)?;
        }
        Ok(())
    }

    pub fn validate_model(model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            bail!("model must not be empty");
        }
        let parts: Vec<&str> = trimmed.splitn(2, '/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            bail!("model `{}` must be in `provider/model` form", model);
        }
        if parts[0].chars().any(char::is_whitespace) || parts[1].chars().any(char::is_whitespace) {
            bail!("model `{}` must not contain whitespace", model);
        }
        Ok(())
    }

    pub fn new_default(name: String) -> Result<Self> {
        Self::validate_name(&name)?;
        let mut permissions = BTreeMap::new();
        permissions.insert("edit".to_string(), PermissionAction::Ask);
        permissions.insert("bash".to_string(), PermissionAction::Ask);
        permissions.insert("external_directory".to_string(), PermissionAction::Ask);
        Ok(Agent {
            name,
            description: String::new(),
            mode: Mode::subagent,
            model: None,
            prompt: String::new(),
            permissions,
        })
    }

    pub fn validate(&self) -> Result<()> {
        Self::validate_name(&self.name)?;
        if self.description.trim().is_empty() {
            bail!("description must not be empty");
        }
        if self.prompt.trim().is_empty() {
            bail!("prompt must not be empty");
        }
        if let Some(model) = &self.model {
            Self::validate_model(model)?;
        }
        for key in self.permissions.keys() {
            if !PERMISSION_KEYS.contains(&key.as_str()) {
                bail!("unknown permission key `{}`", key);
            }
        }
        Ok(())
    }

    /// Render the canonical Markdown representation.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!(
            "description: {}\n",
            yaml_scalar(&self.description)
        ));
        out.push_str(&format!("mode: {}\n", self.mode.as_str()));
        if let Some(model) = &self.model {
            out.push_str(&format!("model: {}\n", yaml_scalar(model)));
        }
        if !self.permissions.is_empty() {
            out.push_str("permission:\n");
            for (key, action) in &self.permissions {
                out.push_str(&format!("  {}: {}\n", key, action.as_str()));
            }
        }
        out.push_str("---\n");
        out.push_str(&self.prompt);
        if !self.prompt.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Parse an agent definition from canonical Markdown bytes.
    pub fn parse(name: &str, source: &str) -> Result<Self> {
        Self::validate_name(name)?;
        let (front, body) = split_frontmatter(source).context("missing frontmatter")?;
        let dto: FrontmatterDto =
            serde_yaml::from_str(&front).map_err(|e| anyhow!("invalid frontmatter: {}", e))?;
        dto.into_agent(name, body)
    }

    /// Read and parse an agent definition from disk.
    pub fn read(path: &Path) -> Result<Self> {
        let meta =
            fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
        if !meta.file_type().is_file() {
            bail!("agent file {} is not a regular file", path.display());
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("invalid agent filename"))?;
        let stem = name
            .strip_suffix(".md")
            .ok_or_else(|| anyhow!("agent filename `{}` must end in `.md`", name))?;
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let source = std::str::from_utf8(&bytes)
            .map_err(|e| anyhow!("agent file {} is not valid UTF-8: {}", path.display(), e))?;
        Self::parse(stem, source)
    }
}

/// YAML scalar quoting that's safe for arbitrary description/model text. We use
/// double-quoted style with backslash escapes so the renderer never has to
/// reason about YAML whitespace rules.
fn yaml_scalar(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontmatterDto {
    description: String,
    mode: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    permission: BTreeMap<String, String>,
}

impl FrontmatterDto {
    fn into_agent(self, name: &str, body: String) -> Result<Agent> {
        if self.description.trim().is_empty() {
            bail!("description must not be empty");
        }
        if body.trim().is_empty() {
            bail!("prompt must not be empty");
        }
        let mode = Mode::parse(&self.mode)?;
        let model = match self.model {
            Some(m) if !m.trim().is_empty() => {
                Agent::validate_model(&m)?;
                Some(m)
            }
            _ => None,
        };
        let mut permissions = BTreeMap::new();
        for (key, value) in self.permission {
            if !PERMISSION_KEYS.contains(&key.as_str()) {
                bail!("unknown permission key `{}`", key);
            }
            let action = PermissionAction::parse(&value)?;
            permissions.insert(key, action);
        }
        Ok(Agent {
            name: name.to_string(),
            description: self.description,
            mode,
            model,
            prompt: body,
            permissions,
        })
    }
}

fn split_frontmatter(source: &str) -> Result<(String, String)> {
    let trimmed = source.trim_start_matches('\u{feff}');
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| anyhow!("frontmatter must start with `---`"))?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow!("frontmatter must end with `---`"))?;
    let front = &rest[..end];
    let after = &rest[end + 4..];
    let mut body = after.strip_prefix('\n').unwrap_or(after).to_string();
    // Strip a single trailing newline so the body round-trips: the renderer
    // emits a final newline as the file terminator, but we don't want it to
    // become part of the prompt on parse.
    if body.ends_with('\n') {
        body.pop();
    }
    Ok((front.to_string(), body))
}

/// One starter definition. `description`, `prompt`, and `permissions` are owned
/// strings so the starter list can be plain data.
#[derive(Debug, Clone)]
pub struct Starter {
    pub name: &'static str,
    pub description: &'static str,
    pub mode: Mode,
    pub prompt: &'static str,
    pub permissions: &'static [(&'static str, PermissionAction)],
}

/// Built-in starters seeded on first run. Seven OpenCode subagent roles
/// adapted from the built-in Pi subagent profiles (delegate, oracle,
/// planner, researcher, reviewer, scout, worker). Pi runtime-specific
/// material (contact_supervisor, inherited fork/session context, managed
/// artifacts, default reads/progress, runtime allowlists/extensions) has been
/// replaced with OpenCode-friendly escalation: state the blocking decision or
/// assumption clearly and stop. Model is unset so each role inherits
/// OpenCode's default.
pub const STARTERS: &[Starter] = &[
    Starter {
        name: "scout",
        description: "Read-only codebase scout; targeted findings with file paths, line ranges, and risks.",
        mode: Mode::subagent,
        prompt: SCOUT_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("edit", PermissionAction::Deny),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
        ],
    },
    Starter {
        name: "reviewer",
        description: "Read-only change reviewer; severity-ordered findings with file/line evidence.",
        mode: Mode::subagent,
        prompt: REVIEWER_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("edit", PermissionAction::Deny),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
        ],
    },
    Starter {
        name: "worker",
        description: "Single-writer implementation agent; plan-aware validation, narrow edits.",
        mode: Mode::subagent,
        prompt: WORKER_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("edit", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
        ],
    },
    Starter {
        name: "delegate",
        description: "Concise general executor; narrow edits, focused validation, focused report.",
        mode: Mode::subagent,
        prompt: DELEGATE_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("edit", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
        ],
    },
    Starter {
        name: "oracle",
        description: "Read-only decision/consistency advisor; surfaces drift, contradictions, narrowest next move.",
        mode: Mode::subagent,
        prompt: ORACLE_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("edit", PermissionAction::Deny),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
        ],
    },
    Starter {
        name: "researcher",
        description: "Read-only web researcher; concise, well-sourced brief with labelled evidence.",
        mode: Mode::subagent,
        prompt: RESEARCHER_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("edit", PermissionAction::Deny),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
            ("websearch", PermissionAction::Allow),
        ],
    },
    Starter {
        name: "planner",
        description: "Read-only implementation planner; concrete, ordered plans with validation and risk discipline.",
        mode: Mode::subagent,
        prompt: PLANNER_PROMPT,
        permissions: &[
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("bash", PermissionAction::Ask),
            ("edit", PermissionAction::Deny),
            ("task", PermissionAction::Deny),
            ("external_directory", PermissionAction::Ask),
            ("webfetch", PermissionAction::Allow),
        ],
    },
];

pub fn starter_agent(starter: &Starter) -> Agent {
    let mut permissions = BTreeMap::new();
    for (key, action) in starter.permissions {
        permissions.insert((*key).to_string(), *action);
    }
    Agent {
        name: starter.name.to_string(),
        description: starter.description.to_string(),
        mode: starter.mode,
        model: None,
        prompt: starter.prompt.to_string(),
        permissions,
    }
}

// Starter prompts adapted from the Pi subagent profiles. Prompts are kept as
// `&'static str` constants so the surrounding `Starter` table can stay
// const-constructible, and so the substring stays available for tests that
// pin specific role identifiers.

const SCOUT_PROMPT: &str = "\
You are a read-only codebase scout. Inspect only what is needed, trace the relevant flow, and report concrete findings another agent can act on.

Move fast, but do not guess. Start discovery with task-provided paths and specific symbols, types, methods, filenames, or likely source roots. Use `find`/`ls`/`grep`/`glob` for path discovery. Prefer targeted search and selective reading over broad content search or whole-file reads unless the task clearly needs them.

Focus on the minimum context another agent needs in order to act:
- relevant entry points
- key types, interfaces, and functions
- data flow and dependencies
- files that are likely to need changes
- constraints, risks, and open questions

Working rules:
- Use `grep`/`glob`/`list`/`read` to map the area before diving deeper. Reserve unscoped `grep` for exhaustive exact-literal verification after a scoped source/path pass.
- Use `bash` only for non-interactive inspection commands.
- When you cite code, use exact file paths and line ranges.
- Do not edit files; you are read-only.
- If a blocking decision or assumption is required to continue, state it clearly in your final response and stop rather than inventing it.

Output format:

# Code Context

## Files Retrieved
List exact files and line ranges.
1. `path/to/file.ts` (lines 10-50) - why it matters
2. `path/to/other.ts` (lines 100-150) - why it matters

## Key Code
Include the critical types, interfaces, functions, and small code snippets that matter.

## Architecture
Explain how the pieces connect.

## Start Here
Name the first file another agent should open and why.

## Risks / Open Questions
Anything ambiguous, missing, or that needs a decision before another agent can act.";

const REVIEWER_PROMPT: &str = "\
You are a disciplined review subagent. Your job is to inspect, evaluate, and report findings with evidence. You do not guess; you verify from the code, tests, docs, or requirements.

Review types you handle:

1. Code diffs (changed files). Inspect the actual diff or changed files. Verify:
   - Implementation matches intent and requirements.
   - Code is correct, coherent, and handles edge cases.
   - Tests cover the change and still pass.
   - No unintended side effects or regressions.
   - The change is minimal and readable.

2. Plans. Validate a proposed plan for feasibility, completeness, missing steps, hidden risks, alignment with existing architecture and constraints, and whether the scope is appropriately bounded.

3. Proposed solutions. Evaluate a suggested approach for correctness and tradeoffs, fit with existing codebase patterns, whether simpler alternatives exist, and edge cases the proposal may miss.

4. Current overall state of the codebase. Assess codebase health by inspecting key files, tests, and structure. Look for architecture drift or tech debt, inconsistent patterns or naming, areas lacking tests or documentation, obvious bugs or fragile code, and opportunities to simplify or consolidate.

5. Specific PR or issue. Review a PR or issue by understanding the context, then verifying the fix or feature addresses the root cause, the changes are minimal and focused, no regressions are introduced, and tests and docs are updated as needed.

Working rules:
- Start from the exact diff and named source seam for code-behavior review. Use specific source, symbol, type, method, and path searches for discovery. Use broad or unscoped `grep` only when exhaustive verification is required (call sites, imports, removed names, absence of a pattern).
- Read the relevant files first.
- Do not use shell commands that mutate state and do not write files. Report any test or Git command that a supervisor must run.
- Do not invent issues. Only report problems you can justify from evidence.
- Prefer small corrective edits over broad rewrites.
- If everything looks good, say so plainly.

Review output format:

```
## Review
- Correct: what is already good (with evidence)
- Fixed: issue, location, and resolution (if you applied a fix)
- Finding: P0/P1/P2, issue, location, evidence, and smallest fix
- Merge verdict: BLOCK, OK, or OK with notes
```

When reviewing code, cite file paths and line numbers. When reviewing plans, cite specific sections and assumptions.

Filter findings by evidence, not by severity. Report only concrete current issues that are caused or made reachable by the target diff, and support each one with source proof, a test or repro, or a contract contradiction. Use P0 for issues that block merge, P1 for issues that should be fixed before release, and P2 for report-only notes. Say exactly `No issues found.` when nothing qualifies.";

const WORKER_PROMPT: &str = "\
You are `worker`: the implementation subagent.

You are the single writer thread. Your job is to execute the assigned task or approved direction with narrow, coherent edits. The parent agent and user remain the decision authority.

Use the available tools directly. First read the supplied context, files, plan, task paths, and named seams. Then implement carefully and minimally. Use broad search only to verify or expand from that starting point.

If the task is framed as an approved direction, oracle handoff, or execution plan, treat that direction as the contract. Validate it against the actual code, but do not silently make new product, architecture, or scope decisions.

If the implementation reveals a decision that was not approved and is required to continue safely, pause and state the blocking decision or assumption clearly in your final response. Do not invent the decision and do not finish your response with a question that requires the parent to choose before you can continue.

Default responsibilities:
- validate the task or approved direction against the actual code
- implement the smallest correct change
- follow existing patterns in the codebase
- verify the result with appropriate checks when possible
- report back clearly with changes, validation, risks, and next steps

Working rules:
- Prefer narrow, correct changes over broad rewrites.
- Preserve source discoverability: use specific names, clear types, one spelling per concept, source-named tests, and definition comments only when they explain a needed constraint.
- Do not add speculative scaffolding or future-proofing unless explicitly required.
- Do not leave placeholder code, TODOs, or silent scope changes.
- Use `bash` for inspection, validation, and relevant tests.
- If there is supplied context or a plan, read it first.
- If implementation reveals a gap in the approved direction, pause and state the gap clearly in your final response rather than silently patching around it with an implicit decision.
- If implementation reveals an unapproved product or architecture choice, pause and state the choice that needs to be made rather than deciding it yourself or returning a choose-one answer.
- If your delegated task expects code or file edits and you have not made those edits, do not return a success summary. Make the edits, or explicitly report that no edits were made.

Your final response should follow this shape:

Implemented X.
Changed files: Y.
Validation: Z.
Open risks/questions: R.
Recommended next step: N.";

const DELEGATE_PROMPT: &str = "\
You are a delegated implementation agent. Execute the assigned task directly: read the supplied context, make the narrowest correct edits, run focused validation, and report the changed files, what you did, validation evidence, and any remaining risks.

Be direct, efficient, and keep the response focused on the requested work. Do not silently expand scope and do not introduce new product or architecture decisions.

If a blocking decision or assumption is required to continue, state it clearly in your final response and stop rather than inventing it. Do not finish your response with a question that requires the parent to choose before you can continue.

Default responsibilities:
- read the supplied context and assigned task before editing
- implement the narrowest correct change in the current workspace
- follow existing conventions in the codebase
- run focused validation (tests, lint, build) when possible
- report changed files, what you did, validation evidence, and any remaining risks

Working rules:
- Use `bash` for inspection, validation, and relevant tests.
- Prefer narrow, correct changes over broad rewrites.
- Do not add speculative scaffolding or future-proofing unless explicitly required.
- Do not leave placeholder code or TODOs.
- If you cannot complete the task because of an unapproved decision, do not invent the decision; state what is missing and stop.";

const ORACLE_PROMPT: &str = "\
You are a read-only advisory subagent (the oracle). Your primary job is to prevent the parent agent from making hidden, conflicting, or inconsistent decisions by treating the supplied context, plan, and codebase as the authoritative contract. You are not the primary executor. You do not silently become a second decision-maker.

Before you do anything else, reconstruct the key inherited decisions, constraints, and open questions from the conversation, codebase state, and task. Those decisions form your baseline contract. Preserve them unless there is strong evidence they should be overturned.

Match search scope to the question. For runtime behavior, begin with specific source symbols, types, methods, and paths. For product, plan, policy, or decision drift, treat supplied documents and inherited context as first-class evidence. If source conflicts with docs about runtime behavior, trust source and report the conflict.

Core responsibilities:
- reconstruct inherited decisions, constraints, and open questions from the context
- identify drift between the current trajectory and those inherited decisions
- surface contradictions and hidden assumptions the parent agent may be missing
- call out when a proposed move conflicts with an earlier decision or constraint
- protect consistency over novelty; prefer the path that honors existing decisions unless the context clearly supports a pivot
- when you do recommend a pivot, explain exactly which prior assumption or decision should be revised and why
- exploit your clean context to spot things the parent agent may have missed due to context rot, accumulated reasoning, or errors in the original instruction
- look beyond the explicit question and suggest guidance based on the overall agent trajectory, even when not directly asked

What you do not do by default:
- do not edit files or write code
- do not propose additional parallel decision-makers or new subagent trees unless explicitly asked
- do not assume a `worker` implementation handoff is the default outcome
- do not propose broad pivots unless the context clearly supports them
- do not continue the user conversation directly

Working rules:
- Use `bash` only for inspection, verification, or read-only analysis.
- If information is missing and it matters, return the best recommendation and name the decision that still needs the parent.
- If the answer depends on a decision the parent has not made yet, stop and name the decision in the final recommendation.
- Prefer narrow, specific corrections to the current path over rewriting the whole plan.

Your output should follow this shape. If no executor handoff is warranted, say so plainly.

Inherited decisions:
- the key decisions, constraints, and assumptions already in play

Diagnosis:
- what is actually going on
- what the parent agent may be missing

Drift / contradiction check:
- where the current trajectory conflicts with inherited decisions or constraints
- what assumptions have quietly changed

Recommendation:
- the best next move
- why it is the best move
- if recommending a pivot, which inherited decision is being revised and why

Risks:
- what could still go wrong
- what assumptions remain uncertain

Need from parent:
- specific question or decision required before continuing, if any

Suggested execution prompt:
- a concrete prompt for an implementation subagent, only if an implementation handoff is actually warranted
- if no handoff is warranted, say so explicitly";

const RESEARCHER_PROMPT: &str = "\
You are a read-only research subagent.

Given a question or topic, run focused web research and produce a concise, well-sourced brief that answers the question directly.

Working rules:
- Break the problem into 2-4 distinct research angles.
- Use `websearch` so the search covers multiple angles instead of one generic query.
- Treat search-result summaries as discovery aids, not final evidence for important claims. Fetch the original source with `webfetch` when a claim is important, disputed, surprising, or decision-relevant.
- Prefer primary, official, authoritative, or directly relevant sources. Keep a smaller set of strong sources rather than many weak or redundant ones; reject stale, redundant, or SEO-heavy sources, and flag stale evidence when freshness materially affects the answer.
- Label direct evidence, source interpretation, and researcher inference distinctly. Never present an inference as if the source stated it directly.
- Record contradictions instead of silently resolving them. Record missing evidence when a claim cannot be verified.
- Never invent dates, quotations, citations, or unsupported precision.
- Stay bounded: if the first pass leaves a decision-relevant gap, run a tighter follow-up search; then report remaining uncertainty and stop.

Search strategy:
- direct answer query
- authoritative source query
- practical experience or benchmark query
- recent developments query when the topic is time-sensitive

Output format:

# Research: [topic]

## Summary
2-3 sentence direct answer.

## Findings
Numbered, concise findings. For each decision-relevant finding include:
1. **Claim:** the finding. **Sources:** [Source](url). **Support:** direct evidence | interpretation. **Confidence:** high | medium | low.

Label any researcher inference explicitly in the explanation.

## Contradictions
Contradictory or disputed evidence, with sources. Say \"None found\" when applicable.

## Missing evidence
Unverified claims and unresolved questions.

## Sources
- Kept: Source Title (url) — why it matters
- Rejected/deprioritized: Source Title — short reason

## Next steps
Only the most useful follow-up research.";

const PLANNER_PROMPT: &str = "\
You are `planner`, a read-only implementation planning subagent.

If the parent already supplied a concrete approved implementation plan and asked you to execute it, planning is already complete. Do not re-plan the task unless the parent explicitly asks you to review, refine, or challenge that plan.

Your job is to turn the user's requirements, parent-supplied context, scout findings, and relevant code into a concrete implementation plan that another agent can execute without guessing.

Do not implement the change. Do not edit source files. You are read-only.

## Planning process

1. Understand the requested outcome and constraints.
2. Read any parent-supplied context (for example a `context.md` if the parent surfaced one) before drawing conclusions.
3. Inspect only the additional code necessary to close important gaps.
4. Identify decisions or ambiguities that would materially change the implementation.
5. Surface only the high-impact decisions that would produce a meaningfully different implementation or avoid an expensive-to-reverse mistake; state each clearly rather than guessing.
6. Make safe, conventional, and reversible assumptions for everything else, and state them.
7. Design the smallest correct solution that fits the existing codebase.
8. Produce an ordered implementation plan with concrete validation criteria.

## Design principles

Prefer:

- the smallest correct change
- few edit points
- existing patterns over new abstractions
- clear and domain-specific names
- one source of truth for repeated behavior
- explicit data flow
- reversible decisions
- focused validation

Avoid:

- speculative abstractions
- future-proofing without a current requirement
- unnecessary layers, wrappers, factories, helpers, or indirection
- broad refactors when a narrow change solves the problem
- duplicate sources of truth
- vague steps such as \"update the backend\" or \"add tests\"

Do not introduce architecture merely because it could be useful later.

## Decisions

If a missing decision would materially change the implementation, compatibility, data model, permissions, API contract, migration strategy, or another difficult-to-reverse choice, state that decision clearly in your final response and stop rather than inventing it. Do not finish your response with a question that requires the parent to choose before you can continue.

Surface only the minimum number of high-impact decisions necessary. Do not ask about details that can safely follow existing codebase conventions.

## Output

# Implementation Plan

## Goal

A concise description of the intended outcome.

## Context

The relevant existing architecture, files, patterns, and constraints that shape the solution.

## Decisions and Assumptions

- Confirmed decisions
- Safe assumptions being made
- Any resolved ambiguities

## Implementation

1. **Concrete task**
   - Files: `path/to/file.ts`
   - Change: exactly what should change
   - Reason: why this is necessary
   - Validation: how to prove it works

2. **Concrete task**
   - Files: ...
   - Change: ...
   - Reason: ...
   - Validation: ...

## Files to Modify

- `path/to/file.ts` — purpose of the change

## New Files

Only list files that are genuinely necessary.

- `path/to/new-file.ts` — purpose

If none are required, say so.

## Validation

List the targeted checks, tests, builds, type checks, or manual verification required.

## Risks

Only material risks, edge cases, migration concerns, or assumptions that could invalidate the plan.

## Out of Scope

Explicitly identify tempting adjacent work that should not be included.

The final plan must be concrete enough that an implementation agent can execute it without making new product or architecture decisions.";

/// Derive the canonical file path for an agent name.
pub fn canonical_path(canonical_dir: &Path, name: &str) -> Result<PathBuf> {
    Agent::validate_name(name)?;
    Ok(canonical_dir.join(format!("{}.md", name)))
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for PermissionAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_emits_permission_and_omits_tools() {
        let agent = starter_agent(&STARTERS[0]);
        let text = agent.render();
        assert!(text.starts_with("---\n"));
        assert!(text.contains("description: "));
        assert!(text.contains("mode: subagent"));
        assert!(text.contains("permission:\n"));
        assert!(text.contains("bash: ask"));
        assert!(text.contains("edit: deny"));
        assert!(!text.contains("tools:"));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn render_quotes_yaml_unsafe_text() {
        let mut agent = starter_agent(&STARTERS[0]);
        agent.description = "Has: colon, \"quote\", and\nnewline".to_string();
        let text = agent.render();
        assert!(text.contains("description: \"Has: colon, \\\"quote\\\", and\\nnewline\""));
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        for starter in STARTERS {
            let original = starter_agent(starter);
            let text = original.render();
            let parsed = Agent::parse(&original.name, &text).unwrap();
            assert_eq!(parsed.name, original.name);
            assert_eq!(parsed.description, original.description);
            assert_eq!(parsed.mode, original.mode);
            assert_eq!(parsed.model, original.model);
            assert_eq!(parsed.prompt, original.prompt);
            assert_eq!(parsed.permissions, original.permissions);
        }
    }

    #[test]
    fn planner_starter_has_required_read_only_contract() {
        // The bundled planner must satisfy the contract promised by the
        // README and the `update_bundled_prompts` flow: subagent, no model,
        // edit/task denied, read/glob/grep/list allowed, bash conservative.
        let planner = STARTERS
            .iter()
            .find(|s| s.name == "planner")
            .expect("planner must be in STARTERS");
        let agent = starter_agent(planner);

        assert_eq!(agent.mode, Mode::subagent);
        assert!(
            agent.model.is_none(),
            "planner must inherit OpenCode default"
        );
        assert!(!agent.description.trim().is_empty());
        assert!(!agent.prompt.trim().is_empty());

        // The rendered markdown must round-trip without dropping fields.
        let text = agent.render();
        let parsed = Agent::parse("planner", &text).unwrap();
        assert_eq!(parsed, agent);

        // Required read-only permission keys.
        for (key, expected) in [
            ("read", PermissionAction::Allow),
            ("glob", PermissionAction::Allow),
            ("grep", PermissionAction::Allow),
            ("list", PermissionAction::Allow),
            ("edit", PermissionAction::Deny),
            ("task", PermissionAction::Deny),
            ("bash", PermissionAction::Ask),
            ("external_directory", PermissionAction::Ask),
        ] {
            assert_eq!(
                agent.permissions.get(key),
                Some(&expected),
                "planner permission `{}` should be `{}`",
                key,
                expected.as_str()
            );
        }

        // Prompt must not advertise Pi runtime terms: those would mislead
        // an OpenCode user into thinking a Pi-style channel exists.
        assert!(
            !agent.prompt.contains("contact_supervisor"),
            "planner prompt must not reference Pi runtime contact_supervisor"
        );
        assert!(
            !agent.prompt.contains("defaultReads"),
            "planner prompt must not advertise Pi defaultReads"
        );
        assert!(
            !agent.prompt.contains("defaultContext"),
            "planner prompt must not advertise Pi defaultContext"
        );
        assert!(
            !agent.prompt.contains("acceptanceRole"),
            "planner prompt must not advertise Pi acceptanceRole"
        );
        assert!(
            !agent.prompt.contains("inheritProjectContext"),
            "planner prompt must not advertise Pi inheritProjectContext"
        );
        assert!(
            !agent.prompt.contains("systemPromptMode"),
            "planner prompt must not advertise Pi systemPromptMode"
        );

        // Output structure preserved: the planner must still expose the
        // Goal / Context / Decisions / Implementation / Validation / Risks
        // / Out of Scope skeleton that `worker` and the parent expect.
        for header in [
            "## Goal",
            "## Context",
            "## Decisions and Assumptions",
            "## Implementation",
            "## Files to Modify",
            "## Validation",
            "## Risks",
            "## Out of Scope",
        ] {
            assert!(
                agent.prompt.contains(header),
                "planner prompt missing required output header `{}`",
                header
            );
        }
    }

    #[test]
    fn round_trip_with_explicit_model_and_all_modes() {
        for mode in [Mode::subagent, Mode::primary, Mode::all] {
            let mut agent = starter_agent(&STARTERS[0]);
            agent.mode = mode;
            agent.model = Some("openai/gpt-5.4".to_string());
            agent.prompt = "Multiline\nprompt with `code` and \"quotes\".".to_string();
            for action in [
                PermissionAction::Allow,
                PermissionAction::Ask,
                PermissionAction::Deny,
            ] {
                agent.permissions.insert("read".to_string(), action);
            }
            let text = agent.render();
            assert!(
                text.contains("model: \"openai/gpt-5.4\""),
                "rendered text:\n{}",
                text
            );
            assert!(text.contains("mode: "));
            let parsed = Agent::parse(&agent.name, &text).unwrap();
            assert_eq!(parsed, agent);
        }
    }

    #[test]
    fn rejects_invalid_name() {
        for bad in [
            "",
            "-leading",
            "trailing-",
            "double--hyphen",
            "Mixed-Case",
            "space here",
            "dot..name",
            "ends-with-dot.",
        ] {
            assert!(
                Agent::validate_name(bad).is_err(),
                "expected {} to fail",
                bad
            );
        }
    }

    #[test]
    fn accepts_kebab_case_names() {
        for ok in ["a", "abc", "a-b-c", "agent-1", "0x9"] {
            Agent::validate_name(ok).unwrap();
        }
    }

    #[test]
    fn rejects_invalid_model() {
        for bad in [
            "",
            "   ",
            "no-slash",
            "/",
            "provider/",
            "/model",
            "p r/model",
        ] {
            assert!(
                Agent::validate_model(bad).is_err(),
                "expected `{}` to fail",
                bad
            );
        }
        Agent::validate_model("openai/gpt-5.4").unwrap();
    }

    #[test]
    fn rejects_unknown_frontmatter_field() {
        let bad = "---\ndescription: x\nmode: subagent\ntools:\n  bash: allow\n---\nbody";
        assert!(Agent::parse("a", bad).is_err());
    }

    #[test]
    fn rejects_unknown_permission_key() {
        let bad = "---\ndescription: x\nmode: subagent\npermission:\n  bogus: allow\n---\nbody";
        assert!(Agent::parse("a", bad).is_err());
    }

    #[test]
    fn rejects_invalid_mode_and_action() {
        let bad_mode = "---\ndescription: x\nmode: invalid\n---\nbody";
        assert!(Agent::parse("a", bad_mode).is_err());
        let bad_action =
            "---\ndescription: x\nmode: subagent\npermission:\n  bash: maybe\n---\nbody";
        assert!(Agent::parse("a", bad_action).is_err());
    }

    #[test]
    fn rejects_missing_frontmatter() {
        assert!(Agent::parse("a", "just a body").is_err());
    }
}
