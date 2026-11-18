//! Test-only fixture: a registry of `Starter` definitions sourced
//! from a subset of `agents/*.md` at the repository root.
//! The fixture exists so unit tests have a way to populate a
//! test checkout without re-rendering markdown by hand; the
//! production binary never reads it.
//!
//! Lives at this depth (one level under `src/agent/`) so the
//! `include_str!` paths to the tracked `.md` files match the depth
//! of the historical `src/agent/bundled/mod.rs` module. The smoke
//! test under `tests/tools_install_smoke.rs` re-includes
//! `src/agent.rs` via `#[path = "../src/agent.rs"]`; inlining the
//! `include_str!` directly into `src/agent.rs` would change the
//! relative path resolution and break that re-include. The whole
//! fixture module is gated behind `#[cfg(test)]` so the production
//! binary carries zero starter bytes.

use crate::agent::Agent;

/// Test-only fixture: each starter definition embeds its
/// `agents/<name>.md` source via `include_str!`.
#[derive(Debug, Clone, Copy)]
pub struct Starter {
    pub name: &'static str,
    pub source: &'static str,
}

/// Test-only fixture: parse the markdown into an `Agent` so the
/// tests can drop a starter into the test checkout without
/// duplicating the render logic.
pub fn starter_agent(starter: &Starter) -> Agent {
    Agent::parse(starter.name, starter.source).expect("bundled starter markdown must parse")
}

/// Test-only fixture: six subagent starters plus the `lukateric`
/// primary in stable order. Tests use this fixture to populate a
/// tempdir checkout; the production binary never reads it.
pub const STARTERS: &[Starter] = &[
    Starter {
        name: "scout",
        source: include_str!("../../../agents/scout.md"),
    },
    Starter {
        name: "reviewer",
        source: include_str!("../../../agents/reviewer.md"),
    },
    Starter {
        name: "worker",
        source: include_str!("../../../agents/worker.md"),
    },
    Starter {
        name: "oracle",
        source: include_str!("../../../agents/oracle.md"),
    },
    Starter {
        name: "researcher",
        source: include_str!("../../../agents/researcher.md"),
    },
    Starter {
        name: "planner",
        source: include_str!("../../../agents/planner.md"),
    },
    Starter {
        name: "lukateric",
        source: include_str!("../../../agents/lukateric.md"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Mode;
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            out.push_str(&format!("{:02x}", b));
        }
        out
    }

    /// Pinned SHA-256 hashes, first captured before the bundled registry
    /// moved to the tracked `agents/*.md` markdown files. The .md
    /// files are the parseable source of truth; this table
    /// guarantees that `parse → render` is byte-faithful so the
    /// test fixtures stay observably identical to editing the .md
    /// directly. Refresh only the rows for intentional profile edits;
    /// regenerating other rows would mask unrelated byte-level drift.
    ///
    /// At that migration, the seven subagent prompt SHAs were preserved
    /// from the original baseline. The `rendered` SHAs were regenerated
    /// because the tracked `.md` frontmatter for those subagents was
    /// re-authored (model field populated, permissions re-tuned)
    /// after the original baseline was captured. The `pi` SHA for
    /// `scout` was likewise regenerated because its permission set
    /// changed and the Pi tool list / acceptanceRole now differ.
    /// `lukateric` replaced the retired `orchestrator`, with a new baseline.
    ///
    /// `rendered` SHAs for `oracle`, `planner`, and `lukateric`
    /// were refreshed once more because their `model` frontmatter
    /// field was updated from `openai/gpt-6-sol` to
    /// `openai/gpt-6.1-sol`. The causal proof: with the model
    /// overwritten back to `openai/gpt-6-sol` in memory (no .md
    /// touched), all 24 prompt/render/pi hashes for the 8 agents
    /// match the prior baseline; prompt and pi SHAs for the 3
    /// affected agents are unchanged because neither surface
    /// embeds the model. The 5 other profiles in that registry
    /// were byte-exact against the prior baseline, confirming the
    /// refresh is scoped to the 3 model-metadata edits.
    ///
    /// On 2026-10-04, the authorized five remaining model pins
    /// changed from `minimax/MiniMax-M3` to `openai/gpt-6.1-sol`.
    /// Only their rendered SHAs changed. The causal test below restores
    /// the old model in memory for the remaining applicable profiles.
    ///
    /// The `worker` and `lukateric` rows reflect consolidation of small
    /// tasks into `worker`, including the local QA rules in `lukateric`.
    const BASELINE: &[(&str, &str, &str, &str)] = &[
        (
            "scout",
            "a3df03e896722379ebdac6e385af94638dddf817697b8687b30dcab40119ea16",
            "03032487ee202a626800814f298d12663731014e3d38b9053ec5d59162f97147",
            "36e5f2a337d61ff6f77933077cf5cb06df97b206b7d40324941a39b3b763a672",
        ),
        (
            "reviewer",
            "00926d546e963ff95fe79848ba1b671611649e2a9502ccfd3a48a079946b75a9",
            "3d50df992787398d1789ff5b20ea0a55e01d2486ce16c81189ed50dd8d2da4ae",
            "9c5da669fb825f505bd44dd728c555f7736d49df720135244af7270ee6580643",
        ),
        (
            "worker",
            "de0c75377694aca9a20541b43fe3f60bbe2a2b3f1c44fbae5e5c1c6abb0f1849",
            "e65cbfd9392e222397eff952fcf87c4ecfac09ce252ac532450a38c6891b2655",
            "2d63f95f038f5fcea15fb88175ebf716c632c3a10edeba100d8b1076a5f674ff",
        ),
        (
            "oracle",
            "116038d4fd892c3b793942c9a8950fdac080315137fd1f181f91f7c5c3ddd98a",
            "af9e620cf6c9bd94b93932261706cce1e6a9c44800ac82e888debfe3b4fa1f3e",
            "351d22bb1380736268773cd62a5463b5f7ffbf200d45aaf7098b7f415c4f7330",
        ),
        (
            "researcher",
            "c28f5bb23381e97620c8899bda01e77c578568d76f61fcd4cf28ac618ed6708b",
            "80e096022e6f9f0ad883a40116bf3d84ef5f90ca8c0a278e0d98bcdde298b88b",
            "ab4497076f80a6d62d5a08e6b81d29ba9d6be760fdd959d5e2c1a6223338d2dd",
        ),
        (
            "planner",
            "d18cdc987a28c4e60980a168b34f55a60842f9c0dfc6acc72e5d463ade02fd18",
            "a41071864dbc64d822986fbe910c76c9059b9d33093d639c73dde073b7ee5b15",
            "cf917a7c2e2a6b553074ff11ef616bf747019627c363109a96cb9f8bc8183f87",
        ),
        (
            "lukateric",
            "5ebdac6cb61323ad4d58f29700c93521b4016e1e70eca13d12bf8d01a7d064a0",
            "3d45e41ca16a6d1163bbc79432a45f4fe2312bafd84a206d6a3cf8134407095d",
            "c0b5dc0686f6e71b36279e96130eea05dd6e097baad88697b69c8f18ff35193b",
        ),
    ];

    #[test]
    fn starter_model_updates_preserve_prior_baseline() {
        // Authorized on 2026-10-04: these model pins changed from
        // minimax/MiniMax-M3 to openai/gpt-6.1-sol; no other bytes may drift.
        // `worker` now uses the current baseline below: reverting only
        // its model cannot restore the historical render after the prompt edit.
        let prior_rendered = [
            (
                "scout",
                "2197e0ff1d2d96bf49b4110f4689059996a460a394d75c91910b43a8fc45ea44",
            ),
            (
                "reviewer",
                "5882339496d8903bdda05942e2d309a647236941b08ecd28faace92f6fc45b15",
            ),
            (
                "researcher",
                "0727a9b59fef3acc119ae6685fe931f425277b99d5b91dfeeb693fe6cdf1c082",
            ),
        ];
        assert_eq!(STARTERS.len(), BASELINE.len());
        let mut affected = 0;
        for (starter, (name, prompt_sha, rendered_sha, pi_sha)) in
            STARTERS.iter().zip(BASELINE.iter())
        {
            assert_eq!(starter.name, *name);
            let agent = starter_agent(starter);
            assert_eq!(sha256_hex(agent.prompt.as_bytes()), *prompt_sha);
            assert_eq!(sha256_hex(agent.render_pi().as_bytes()), *pi_sha);
            if let Some((_, old_rendered_sha)) =
                prior_rendered.iter().find(|(old_name, _)| old_name == name)
            {
                affected += 1;
                assert_eq!(agent.model.as_deref(), Some("openai/gpt-6.1-sol"));
                let mut prior_agent = agent.clone();
                prior_agent.model = Some("minimax/MiniMax-M3".into());
                assert_eq!(
                    sha256_hex(prior_agent.render().as_bytes()),
                    *old_rendered_sha
                );
                assert_eq!(sha256_hex(prior_agent.prompt.as_bytes()), *prompt_sha);
                assert_eq!(sha256_hex(prior_agent.render_pi().as_bytes()), *pi_sha);
            } else {
                assert_eq!(sha256_hex(agent.render().as_bytes()), *rendered_sha);
            }
        }
        assert_eq!(affected, prior_rendered.len());
    }

    #[test]
    fn starter_registry_matches_baseline() {
        assert_eq!(STARTERS.len(), BASELINE.len());
        for (starter, (name, prompt_sha, rendered_sha, pi_sha)) in
            STARTERS.iter().zip(BASELINE.iter())
        {
            assert_eq!(starter.name, *name);
            let agent = starter_agent(starter);
            assert_eq!(sha256_hex(agent.prompt.as_bytes()), *prompt_sha);
            assert_eq!(sha256_hex(agent.render().as_bytes()), *rendered_sha);
            assert_eq!(sha256_hex(agent.render_pi().as_bytes()), *pi_sha);
        }
    }

    #[test]
    fn starter_registry_order_is_stable() {
        let names: Vec<&str> = STARTERS.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "scout",
                "reviewer",
                "worker",
                "oracle",
                "researcher",
                "planner",
                "lukateric",
            ]
        );
        // At most one primary starter is expected; if one is present
        // it must come last. The fixture used to pin `orchestrator`
        // as the single primary; after the orchestrator role was
        // retired, `lukateric` is the only tracked primary. The
        // assertion stays loose so a future fixture that drops the
        // primary entirely does not break this test.
        let primaries: Vec<&str> = STARTERS
            .iter()
            .filter(|s| starter_agent(s).mode == Mode::primary)
            .map(|s| s.name)
            .collect();
        assert!(
            primaries.len() <= 1,
            "at most one primary starter is allowed: {primaries:?}"
        );
        if let Some(primary_name) = primaries.first() {
            assert_eq!(*primary_name, "lukateric");
            assert_eq!(*primary_name, *names.last().expect("non-empty"));
        }
    }
}
