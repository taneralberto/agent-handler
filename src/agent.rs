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

    /// Render the Pi subagent definition derived from this canonical agent.
    pub fn render_pi(&self) -> String {
        let mut tools = vec!["read", "grep", "find", "ls", "contact_supervisor"];
        if self.permissions.get("bash") != Some(&PermissionAction::Deny) {
            tools.push("bash");
        }
        if self.permissions.get("edit") != Some(&PermissionAction::Deny) {
            tools.extend(["edit", "write"]);
        }
        if self.permissions.get("task") == Some(&PermissionAction::Allow) {
            tools.push("subagent");
        }

        format!(
            "---\nname: {}\ndescription: {}\ntools: {}\nsystemPromptMode: replace\ninheritProjectContext: true\ninheritSkills: false\nacceptanceRole: {}\n---\n\n{}\n",
            self.name,
            yaml_scalar(&self.description),
            tools.join(", "),
            if self.permissions.get("edit") == Some(&PermissionAction::Allow) {
                "writer"
            } else {
                "read-only"
            },
            self.prompt.replace("OpenCode", "Pi").trim_end(),
        )
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
                let model = m.trim();
                Agent::validate_model(model)?;
                Some(model.to_string())
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
    // Canonical renders use LF. Normalize only CRLF inputs so files edited on
    // Windows parse to the same in-memory agent as their LF equivalent.
    let normalized = source
        .contains("\r\n")
        .then(|| source.replace("\r\n", "\n"));
    let source = normalized.as_deref().unwrap_or(source);

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

// The explicit `#[path]` is required because `tests/tools_install_smoke.rs`
// re-includes this file via `#[path = "../src/agent.rs"]`. Without it,
// `mod bundled;` resolves relative to that include point and looks for
// `src/bundled.rs` / `src/bundled/mod.rs`, which do not exist; the real
// location is `src/agent/bundled/mod.rs`. Pinning the path here keeps both
// the normal crate build and the smoke-test re-include working.
#[path = "agent/bundled/mod.rs"]
mod bundled;
// `Starter` is part of the public API per the bundled-starter module
// contract; suppress the unused_imports lint that fires when no caller
// in this crate currently references the type by name.
#[allow(unused_imports)]
pub use bundled::{starter_agent, Starter, STARTERS};

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
    fn render_pi_uses_pi_frontmatter_and_tools() {
        let scout = starter_agent(&STARTERS[0]);
        let text = scout.render_pi();
        assert!(text.contains("name: scout"));
        assert!(text.contains("tools: read, grep, find, ls, contact_supervisor, bash"));
        assert!(text.contains("acceptanceRole: read-only"));
        assert!(text.contains("You are"));
        assert!(!text.contains("OpenCode"));
        assert!(!text.contains("mode:"));

        let orchestrator = starter_agent(
            STARTERS
                .iter()
                .find(|starter| starter.name == "orchestrator")
                .unwrap(),
        );
        assert!(orchestrator.render_pi().contains("subagent"));
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
    fn parse_normalizes_crlf_document_endings() {
        let source = "---\r\ndescription: x\r\nmode: subagent\r\nmodel: openai/gpt-5.4\r\n---\r\nline one\r\nline two\r\n";

        let parsed = Agent::parse("a", source).unwrap();

        assert_eq!(parsed.model.as_deref(), Some("openai/gpt-5.4"));
        assert_eq!(parsed.prompt, "line one\nline two");
        assert!(!parsed.render().contains('\r'));
    }

    #[test]
    fn parse_trims_model_before_storing_it() {
        let source = "---\ndescription: x\nmode: subagent\nmodel: \" openai/gpt-5.4 \"\n---\nbody";

        let parsed = Agent::parse("a", source).unwrap();

        assert_eq!(parsed.model.as_deref(), Some("openai/gpt-5.4"));
    }

    #[test]
    fn orchestrator_starter_has_primary_coordination_contract() {
        let orchestrator = STARTERS
            .iter()
            .find(|s| s.name == "orchestrator")
            .expect("orchestrator must be in STARTERS");
        let agent = starter_agent(orchestrator);

        assert_eq!(agent.mode, Mode::primary);
        assert!(agent.model.is_none());
        assert_eq!(
            agent.permissions.get("task"),
            Some(&PermissionAction::Allow),
            "orchestrator must be able to delegate"
        );
        assert_eq!(
            agent.permissions.get("edit"),
            Some(&PermissionAction::Deny),
            "orchestrator must leave edits to a worker"
        );
        assert!(agent.prompt.contains("one implementation writer at a time"));
        assert!(agent
            .prompt
            .contains("Do not ask subagents to create further subagent trees."));
        assert_eq!(
            Agent::parse("orchestrator", &agent.render()).unwrap(),
            agent
        );
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
