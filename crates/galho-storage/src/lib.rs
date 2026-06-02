//! galho-storage — content-addressed Merkle-DAG object store + DAG operations.
//!
//! See `pleme-io/theory/GALHO.md` §V. Two layers:
//!
//! - [`ObjectStore`] (§V.2): pluggable backend trait. Stores raw bytes keyed by
//!   BLAKE3 content hash; supports auxiliary meta blobs; provides atomic CAS on named refs.
//!   Backends: [`backends::MemoryBackend`] (in-process, for tests + offline dev),
//!   [`backends::LocalFsBackend`] (canonical local dev backend; lives under
//!   `<root>/{objects,meta,refs}/`).
//!
//! - [`GalhoTree`] (§II.3, §V.4): high-level DAG operations layered over `ObjectStore`.
//!   Insert typed states, advance galho heads via CAS, traverse parent edges,
//!   compute the live set, run reference-counted GC.
//!
//! S3 + Postgres backends land in M3 / M9 respectively (§XII.2). The trait surface
//! is intentionally async to make the M3 transition free of API changes.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod backends;
pub mod compliance_verifier;
pub mod galho_tree;
pub mod object_store;
pub mod outcome_chain;

pub use compliance_verifier::{verify, verify_regime};
pub use galho_tree::{
    AdvanceOutcome, Divergence, GalhoTree, GcReport, NodeMeta, META_KIND_NODE,
};
pub use object_store::{Addressed, ObjectStore, StoreError, put_typed};
pub use outcome_chain::{
    compute_entry_hash, ChainedOutcomeEmitter, ChainedOutcomeEntry, OutcomeChain,
    META_KIND_OUTCOME_INDEX, REF_OUTCOME_HEAD,
};

// Re-export Blake3Hash through one consistent path.
pub use galho_types::Blake3Hash;
