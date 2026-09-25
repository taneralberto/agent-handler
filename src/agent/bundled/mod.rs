//! Built-in starter agents seeded on first run.
//!
//! One primary orchestrator and seven OpenCode subagent roles adapted from
//! Pi subagents. Each role owns its own prompt and permissions under
//! `bundled::<role>`. Pi runtime-specific material (`contact_supervisor`,
//! inherited fork/session context, managed artifacts, default
//! reads/progress, runtime allowlists/extensions) has been replaced with
//! OpenCode-friendly escalation: state the blocking decision or
//! assumption clearly and stop. Model is unset so each role inherits
//! OpenCode's default.

use crate::agent::{Agent, Mode, PermissionAction};
use std::collections::BTreeMap;

pub mod delegate;
pub mod oracle;
pub mod orchestrator;
pub mod planner;
pub mod researcher;
pub mod reviewer;
pub mod scout;
pub mod worker;

/// One starter definition. `description`, `prompt`, and `permissions` are
/// owned strings so the starter list can be plain data.
#[derive(Debug, Clone, Copy)]
pub struct Starter {
    pub name: &'static str,
    pub description: &'static str,
    pub mode: Mode,
    pub prompt: &'static str,
    pub permissions: &'static [(&'static str, PermissionAction)],
}

pub const READ_ONLY_PERMISSIONS: &[(&str, PermissionAction)] = &[
    ("read", PermissionAction::Allow),
    ("glob", PermissionAction::Allow),
    ("grep", PermissionAction::Allow),
    ("list", PermissionAction::Allow),
    ("bash", PermissionAction::Ask),
    ("edit", PermissionAction::Deny),
    ("task", PermissionAction::Deny),
    ("external_directory", PermissionAction::Ask),
    ("webfetch", PermissionAction::Allow),
];

pub const WRITER_PERMISSIONS: &[(&str, PermissionAction)] = &[
    ("read", PermissionAction::Allow),
    ("glob", PermissionAction::Allow),
    ("grep", PermissionAction::Allow),
    ("list", PermissionAction::Allow),
    ("edit", PermissionAction::Allow),
    ("bash", PermissionAction::Ask),
    ("task", PermissionAction::Deny),
    ("external_directory", PermissionAction::Ask),
    ("webfetch", PermissionAction::Allow),
];

pub const STARTERS: &[Starter] = &[
    scout::STARTER,
    reviewer::STARTER,
    worker::STARTER,
    delegate::STARTER,
    oracle::STARTER,
    researcher::STARTER,
    planner::STARTER,
    orchestrator::STARTER,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Mode;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

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

    /// Pinned SHA-256 baseline captured before splitting the bundled
    /// starter definitions out of `src/agent.rs` into
    /// `src/agent/bundled/<role>/mod.rs`. The split is a pure source-code
    /// refactor: every prompt byte, every rendered byte, and every
    /// Pi-rendered byte must round-trip exactly. Do **not** regenerate the
    /// baseline to make this test pass; that would mask a silent byte-level
    /// drift.
    const BASELINE: &[(&str, &str, &str, &str)] = &[
        // (name, prompt_sha256, rendered_sha256, pi_rendered_sha256)
        (
            "scout",
            "a3df03e896722379ebdac6e385af94638dddf817697b8687b30dcab40119ea16",
            "359361dd79bce99f4e81b040d8288b975bee0b712417e7309552df8269121788",
            "ddad80a7c76ea927fefd7faba5323532a1b73e3b3862b4d7e058101c5cd0c892",
        ),
        (
            "reviewer",
            "00926d546e963ff95fe79848ba1b671611649e2a9502ccfd3a48a079946b75a9",
            "bb02203bfcdf24d00eab8126ebfb76710c874fb972b216666b724b0713869a20",
            "9c5da669fb825f505bd44dd728c555f7736d49df720135244af7270ee6580643",
        ),
        (
            "worker",
            "ccffd9c334e29225eca392ce329683f4052c2cb8aa51bb13b41ba00f6d3a9169",
            "c10c2c588d67854e28e8fe9c440474042a76c5751626973103de6b682aa639d9",
            "79bf3be77e83fa323761ffe44ed4ccf343a17c44752932f1f1ee8baa5125e058",
        ),
        (
            "delegate",
            "02b3bc732130d9430bab517da9a080773f8c1d46b03dae4d0fa51b1ddad72d64",
            "aecbf8c2d7d78cb5d83bd003628da94149367bc5f1133b452fba2f049fa16224",
            "b88dafad8334fb3a563bc162eab5ffa1436891acd6506bd199bde823879b8b9a",
        ),
        (
            "oracle",
            "116038d4fd892c3b793942c9a8950fdac080315137fd1f181f91f7c5c3ddd98a",
            "07ff82af17cbcdca9ed0f367f07cf31d37479469eb53832decb94dfd85e7ac0e",
            "351d22bb1380736268773cd62a5463b5f7ffbf200d45aaf7098b7f415c4f7330",
        ),
        (
            "researcher",
            "c28f5bb23381e97620c8899bda01e77c578568d76f61fcd4cf28ac618ed6708b",
            "5ecebae9198f66c816ee66161d47968bb4194f65618205fbdc7134acf52a2cbb",
            "ab4497076f80a6d62d5a08e6b81d29ba9d6be760fdd959d5e2c1a6223338d2dd",
        ),
        (
            "planner",
            "d18cdc987a28c4e60980a168b34f55a60842f9c0dfc6acc72e5d463ade02fd18",
            "97b3cb0c1d361fb912b32f32277598cbf95bee9d059e99c9ef6e528af2d8d054",
            "cf917a7c2e2a6b553074ff11ef616bf747019627c363109a96cb9f8bc8183f87",
        ),
        (
            "orchestrator",
            "ead3cfd79f6d19bd4ddbbf1632bcba4cc6dc034237993fcc4a67bcaa13f7e9f2",
            "770b3b2f315850526b50581fe7b1a511edfcab4890e01a8d47c08c3ce625f4aa",
            "a26a9b3990417a3270c603da230c9949c175ce23c0866176384d4876582abef8",
        ),
    ];

    #[test]
    fn starter_registry_matches_pre_split_baseline() {
        assert_eq!(
            STARTERS.len(),
            BASELINE.len(),
            "STARTERS row count must match the baseline"
        );

        for (starter, (name, prompt_sha, rendered_sha, pi_sha)) in
            STARTERS.iter().zip(BASELINE.iter())
        {
            assert_eq!(starter.name, *name, "starter name must match baseline row");

            let actual_prompt = sha256_hex(starter.prompt.as_bytes());
            assert_eq!(
                actual_prompt, *prompt_sha,
                "prompt bytes for `{name}` drifted from the pre-split baseline"
            );

            let agent = starter_agent(starter);
            let actual_rendered = sha256_hex(agent.render().as_bytes());
            assert_eq!(
                actual_rendered, *rendered_sha,
                "render() bytes for `{name}` drifted from the pre-split baseline"
            );

            let actual_pi = sha256_hex(agent.render_pi().as_bytes());
            assert_eq!(
                actual_pi, *pi_sha,
                "render_pi() bytes for `{name}` drifted from the pre-split baseline"
            );
        }
    }

    /// Sanity: the registry is in the documented order and orchestrator is
    /// the only primary-mode starter.
    #[test]
    fn starter_registry_order_is_stable() {
        let names: Vec<&str> = STARTERS.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "scout",
                "reviewer",
                "worker",
                "delegate",
                "oracle",
                "researcher",
                "planner",
                "orchestrator",
            ],
            "STARTERS order must remain scout, reviewer, worker, delegate, oracle, researcher, planner, orchestrator"
        );

        let mut primaries = STARTERS.iter().filter(|s| s.mode == Mode::primary);
        let primary = primaries.next().expect("one primary starter");
        assert_eq!(primary.name, "orchestrator");
        assert!(
            primaries.next().is_none(),
            "only orchestrator may be Mode::primary"
        );
        assert_eq!(primary.permissions.len(), 8);
    }

    /// Sanity: every starter's `starter_agent` constructor produces an Agent
    /// whose `render()` and `render_pi()` are byte-equal to the Starter's
    /// registered prompt and permissions — i.e. the registry didn't quietly
    /// drop a field.
    #[test]
    fn starter_agent_round_trips_every_starter() {
        for starter in STARTERS {
            let agent = starter_agent(starter);
            assert_eq!(agent.name, starter.name);
            assert_eq!(agent.description, starter.description);
            assert_eq!(agent.mode, starter.mode);
            assert_eq!(agent.model, None);
            assert_eq!(agent.prompt, starter.prompt);

            let mut expected = BTreeMap::new();
            for (k, v) in starter.permissions {
                expected.insert((*k).to_string(), *v);
            }
            assert_eq!(agent.permissions, expected);
        }
    }

    /// Sanity: PI-rendered output for every starter ends with a newline so
    /// the on-disk file is always a well-formed canonical Pi doc.
    #[test]
    fn render_pi_output_is_well_formed_for_every_starter() {
        for starter in STARTERS {
            let agent = starter_agent(starter);
            let text = agent.render_pi();
            assert!(
                text.ends_with('\n'),
                "`{}` pi render must end in newline",
                starter.name
            );
            assert!(text.contains("name: "));
            assert!(text.contains("description: "));
            assert!(text.contains("tools: "));
            assert!(text.contains("acceptanceRole: "));
            // Pi docs must not leak OpenCode-only fields.
            assert!(
                !text.contains("mode:"),
                "`{}` pi render must not contain `mode:`",
                starter.name
            );
        }
    }

    // Per-role/registry parity is a type-system invariant now that
    // `STARTERS` is literally `[scout::STARTER, reviewer::STARTER, ...]`;
    // any divergence would be a compile error.
}
