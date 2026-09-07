use anyhow::{anyhow, bail, Result};
use std::process::Command;

/// Result of attempting to discover models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    /// Discovered at least one model.
    Found(Vec<String>),
    /// Command ran successfully but no valid lines were parsed.
    Empty(String),
    /// Command failed or returned non-zero.
    Failed(String),
}

impl Discovery {
    #[allow(dead_code)]
    pub fn models(&self) -> &[String] {
        match self {
            Discovery::Found(models) => models,
            _ => &[],
        }
    }

    pub fn status_text(&self) -> String {
        match self {
            Discovery::Found(models) => format!("{} model(s) discovered", models.len()),
            Discovery::Empty(msg) => format!("no models parsed: {}", msg),
            Discovery::Failed(msg) => format!("discovery failed: {}", msg),
        }
    }
}

/// Discover available models by running `opencode models`.
pub fn discover_models() -> Discovery {
    match run_opencode_models() {
        Ok(stdout) => {
            let parsed = parse_models(&stdout);
            if parsed.is_empty() {
                Discovery::Empty(if stdout.is_empty() {
                    "command produced no output".to_string()
                } else {
                    "no valid provider/model lines found".to_string()
                })
            } else {
                Discovery::Found(parsed)
            }
        }
        Err(e) => Discovery::Failed(e.to_string()),
    }
}

fn run_opencode_models() -> Result<String> {
    let output = Command::new("opencode")
        .arg("models")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| anyhow!("could not execute `opencode models`: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        bail!(
            "`opencode models` exited with status {}: {}",
            code,
            if stderr.is_empty() {
                "<no stderr>"
            } else {
                &stderr
            }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse line-oriented output into sorted, deduplicated `provider/model`
/// identifiers. Invalid lines are silently dropped.
pub fn parse_models(stdout: &str) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for raw in stdout.lines() {
        // Only strip CR for CRLF line endings; leading/trailing whitespace
        // makes a line invalid.
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with(char::is_whitespace) || line.ends_with(char::is_whitespace) {
            continue;
        }
        if let Ok(()) = validate_identifier(line) {
            set.insert(line.to_string());
        }
    }
    set.into_iter().collect()
}

fn validate_identifier(value: &str) -> Result<()> {
    let parts: Vec<&str> = value.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("not provider/model");
    }
    if parts[0].chars().any(char::is_whitespace) || parts[1].chars().any(char::is_whitespace) {
        bail!("contains whitespace");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_output() {
        let out = "openai/gpt-5.4\nopenai/gpt-5.4-mini\nanthropic/claude-3\n";
        assert_eq!(
            parse_models(out),
            vec![
                "anthropic/claude-3".to_string(),
                "openai/gpt-5.4".to_string(),
                "openai/gpt-5.4-mini".to_string(),
            ]
        );
    }

    #[test]
    fn deduplicates_and_drops_invalid() {
        let out = "openai/gpt-5.4\nopenai/gpt-5.4\nno-slash\n  openai/trim-me  \n\n";
        assert_eq!(parse_models(out), vec!["openai/gpt-5.4".to_string()]);
    }

    #[test]
    fn strips_carriage_return() {
        let out = "openai/gpt-5.4\r\nopenai/gpt-5.4-mini\r\n";
        assert_eq!(
            parse_models(out),
            vec![
                "openai/gpt-5.4".to_string(),
                "openai/gpt-5.4-mini".to_string()
            ]
        );
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        let out = "# header\n\nopenai/gpt-5.4\n   \n";
        assert_eq!(parse_models(out), vec!["openai/gpt-5.4".to_string()]);
    }

    #[test]
    fn accepts_punctuation_in_ids() {
        let out = "openai/gpt-5.4-fast\nopenai/gpt-5.3-codex-spark\n";
        assert_eq!(
            parse_models(out),
            vec![
                "openai/gpt-5.3-codex-spark".to_string(),
                "openai/gpt-5.4-fast".to_string(),
            ]
        );
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_models("").is_empty());
        assert!(parse_models("   \n\n").is_empty());
    }

    #[test]
    fn malformed_input_yields_empty() {
        assert!(parse_models("just a sentence\nanother line\n").is_empty());
    }

    #[test]
    fn status_text_for_each_variant() {
        assert_eq!(
            Discovery::Found(vec!["a/b".into()]).status_text(),
            "1 model(s) discovered"
        );
        assert!(Discovery::Empty("x".into()).status_text().contains("x"));
        assert!(Discovery::Failed("y".into()).status_text().contains("y"));
    }

    #[test]
    fn models_accessor() {
        assert_eq!(
            Discovery::Found(vec!["a/b".to_string()]).models(),
            &["a/b".to_string()]
        );
        assert!(Discovery::Empty("x".into()).models().is_empty());
        assert!(Discovery::Failed("x".into()).models().is_empty());
    }
}
