//! Bundled catalog entry for the `pi-psql` OpenCode skill.
//!
//! Adding another tool is one `pub const ENTRY: ToolCatalogEntry` in a
//! new `tools/<tool>/mod.rs`, plus (only if the default SKILL.md `name:`
//! identity check is insufficient) a tool-specific validation step. The
//! pinned SHA is the authoritative identity; `pin_tag` is the lookup key.
//!
//! `pi-psql` ships first with the verified tag/SHA pair: the peeled
//! commit at `opencode-2026-09-23` is
//! `0dba366061911f0ec389f4a78cc46fd6d6a19d41` and the tag object is
//! `409543fe9750fdcc60fcea858c43c623cc40aaa9`.

use super::{NodeMin, ToolCatalogEntry};

/// The bundled `pi-psql` catalog entry. The shape is the
/// `ToolCatalogEntry` record shared with future tools; only the values
/// here are tool-specific.
pub const ENTRY: ToolCatalogEntry = ToolCatalogEntry {
    repo: "https://github.com/taneralberto/pi-psql.git",
    skill_name: "pi-psql",
    destination_subpath: "pi-psql",
    pin_tag: "opencode-2026-09-23",
    expected_sha: "0dba366061911f0ec389f4a78cc46fd6d6a19d41",
    // Conservative bound that covers the `^22.12.0` leg of yargs@^18's
    // engines.node; matches the existing engines node `^20.19.0 || ^22.12.0
    // || >=23` range conservatively.
    node_min: NodeMin {
        major: 22,
        minor: 12,
        patch: 0,
    },
};
