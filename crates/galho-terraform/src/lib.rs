//! galho-terraform — the canonical reference IaCSystem adapter for Terraform.
//!
//! See `pleme-io/theory/GALHO.md` §III.3. The first concrete consumer of the
//! `IaCSystem` marker trait + the bidirectional translation between Terraform
//! tfstate v4 JSON and galho's canonical Resource Graph IR.
//!
//! v0.1 ships the marker impl + tfstate JSON round-trip via local-fs paths.
//! M3.1 wires magma's `StateBackend` for real terraform integration; until then,
//! `read_state` / `write_state` operate on JSON files.
//!
//! The canonical-bytes-by-construction property holds across the round trip:
//! `tfstate_to_canonical(canonical_to_tfstate(state)) == state` (modulo serde
//! field-order differences). Property-tested in `tests/round_trip.rs`.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

pub mod tfstate;
pub mod translate;

use galho_types::{ApplySemantics, IaCSystem, IaCSystemId};

/// The Terraform IaCSystem marker. Stable identifier `"terraform"`. Schema version
/// `"4"` matches tfstate v4 (the canonical format from Terraform 0.12+).
///
/// Apply semantics: `PartialProgress` — terraform apply may partially succeed; state
/// is updated to reflect what actually applied; the galho runtime interprets per-
/// resource status against this declared semantics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Terraform;

impl IaCSystem for Terraform {
    fn id() -> IaCSystemId {
        IaCSystemId::new("terraform")
    }

    fn schema_version() -> &'static str {
        "4"
    }

    fn apply_semantics() -> ApplySemantics {
        ApplySemantics::PartialProgress
    }
}

pub use tfstate::{Tfstate, TfstateResource, TfstateInstance, TfstateError};
pub use translate::{tfstate_to_canonical, canonical_to_tfstate, TranslateError};
