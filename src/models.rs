use anyhow::{anyhow, bail, Result};
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::io::ErrorKind;
#[cfg(windows)]
use std::path::PathBuf;
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
    // `spawn_opencode_models` runs the original
    // `Command::new("opencode")` and only on Windows falls back to
    // the npm `opencode.cmd` shim if the bare-name attempt returns
    // `NotFound` — see that helper for the why and how.
    let output = spawn_opencode_models()
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

/// Spawn `opencode models`. Windows-only fallback: Rust's bare
/// executable lookup doesn't append `.cmd`, so when only the npm
/// `opencode.cmd` shim is on PATH, `Command::new("opencode")` fails
/// with `NotFound`. We locate the shim and re-spawn via its full path;
/// Rust's std handles explicitly named batch shims internally, so we
/// never assemble a `cmd.exe /C` command line ourselves (avoids
/// cmd.exe quoting pitfalls around `&` in paths). Non-Windows and
/// non-`NotFound` errors propagate as-is.
fn spawn_opencode_models() -> std::io::Result<std::process::Output> {
    match Command::new("opencode")
        .arg("models")
        .env("NO_COLOR", "1")
        .output()
    {
        Ok(out) => Ok(out),
        #[cfg(windows)]
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let path = std::env::var_os("PATH").unwrap_or_default();
            match locate_opencode_cmd_in(&path) {
                Some(shim) => Command::new(&shim)
                    .arg("models")
                    .env("NO_COLOR", "1")
                    .output(),
                None => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// Locate the npm-shipped `opencode.cmd` shim by walking `PATH`.
/// Needed because Rust's Windows bare executable lookup doesn't
/// append `.cmd`. Pure so they don't have to touch process state in
/// tests. Real `opencode.exe` installs are resolved by the bare-name
/// spawn and never hit this path.
#[cfg(windows)]
fn locate_opencode_cmd_in(path: &OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path) {
        let candidate = dir.join("opencode.cmd");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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

    // Windows-only: walks PATH for `opencode.cmd`. Synthesized
    // shims in isolated TempDirs keep the host PATH untouched.
    #[cfg(windows)]
    #[test]
    fn locate_opencode_cmd_in_finds_shim_even_when_dir_has_amper() {
        // Directory name intentionally contains `&` (e.g.
        // `C:\R&D\bin\opencode.cmd`). The resolved path goes to
        // `Command::new`, never to a shell.
        let dir = tempfile::tempdir().expect("tempdir");
        let weird = dir.path().join("with&here");
        std::fs::create_dir_all(&weird).expect("create weird dir");
        let shim = weird.join("opencode.cmd");
        std::fs::write(&shim, b"").expect("write shim");
        let path_env = std::env::join_paths([&weird]).expect("PATH env");

        assert_eq!(
            locate_opencode_cmd_in(path_env.as_os_str()),
            Some(shim),
            "locator must return the shim even when its parent dir contains `&`"
        );
    }

    #[cfg(windows)]
    #[test]
    fn locate_opencode_cmd_in_returns_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_env = std::env::join_paths([dir.path()]).expect("PATH env");
        assert!(
            locate_opencode_cmd_in(path_env.as_os_str()).is_none(),
            "no opencode.cmd in PATH -> locator must return None"
        );
    }

    #[cfg(windows)]
    #[test]
    fn locate_opencode_cmd_in_honours_path_order() {
        let first = tempfile::tempdir().expect("first tempdir");
        let second = tempfile::tempdir().expect("second tempdir");
        std::fs::write(first.path().join("opencode.cmd"), b"").expect("write first");
        std::fs::write(second.path().join("opencode.cmd"), b"").expect("write second");
        let path_env = std::env::join_paths([first.path(), second.path()]).expect("PATH env");

        let found = locate_opencode_cmd_in(path_env.as_os_str()).expect("located");
        assert!(
            found.starts_with(first.path()),
            "earlier PATH directory must win, got: {}",
            found.display()
        );
    }

    // End-to-end: a `.cmd` shim at a path containing `&` runs to
    // completion via `Command::new(<full .cmd path>)` — proving
    // the fallback shape does not regress on real cmd.exe quoting
    // traps. No host PATH mutation; the shim lives in an isolated
    // TempDir and we pass it to `Command::new` ourselves.
    #[cfg(windows)]
    #[test]
    fn command_new_full_cmd_path_handles_amper_in_dirname() {
        let dir = tempfile::tempdir().expect("tempdir");
        let weird = dir.path().join("with&here");
        std::fs::create_dir_all(&weird).expect("create weird dir");
        let shim = weird.join("opencode.cmd");
        std::fs::write(&shim, b"@echo openai/gpt-5.4\r\n").expect("write shim");

        let out = Command::new(&shim)
            .arg("models")
            .env("NO_COLOR", "1")
            .output()
            .expect("Command::new(<full .cmd path>) must run the shim");
        assert!(out.status.success(), "shim exited non-zero: {:?}", out);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "openai/gpt-5.4",
            "shim stdout should reach us verbatim, even from a path containing `&`"
        );
    }
}
