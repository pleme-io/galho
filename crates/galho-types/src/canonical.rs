//! Canonical-bytes emission for galho IR types.
//!
//! galho consumes `tameshi::canonicalize::Canonicalizer` for the BLAKE3 hashing pass.
//! This module exposes the IR-side primitive: [`CanonicalBytes`] — a small typed trait
//! that walks an IR value and emits a canonical byte stream into a [`CanonicalSink`].
//! The emitted bytes are canonical-by-construction (sorted maps, length-prefixed,
//! tag-typed), so we feed them through tameshi's `RawCanonicalizer` (identity passthrough).
//!
//! Three reasons a galho-side trait exists rather than just using serde-cbor or DAG-CBOR
//! directly:
//!
//! 1. Identity-vs-metadata field separation. `Resource.provenance` is recording-only
//!    metadata; `Resource.id` is identity. The trait gives us per-field control.
//! 2. `SecretRef` must emit its reference, never its resolved value — a typed discipline
//!    easier to enforce in a galho-owned trait than via serde attributes.
//! 3. Per-attribute order semantics (some lists are order-sensitive, some aren't) are
//!    galho-IR-specific and would not generalize back into tameshi.

use std::collections::BTreeMap;
use tameshi::canonicalize::{CanonicalMode, RawCanonicalizer, canonical_hash};
use tameshi::hash::Blake3Hash;

/// One-byte tags identifying each IR-emitted construct. Tags participate in the
/// canonical hash; changing a tag value invalidates every existing hash and is a
/// schema-version bump.
pub mod tag {
    pub const NULL: u8 = 0x00;
    pub const BOOL: u8 = 0x01;
    pub const INT: u8 = 0x02;
    pub const FLOAT: u8 = 0x03;
    pub const STRING: u8 = 0x04;
    pub const BYTES: u8 = 0x05;
    pub const LIST: u8 = 0x06;
    pub const MAP: u8 = 0x07;
    pub const SECRET_REF: u8 = 0x08;
    pub const CROSS_SYS_REF: u8 = 0x09;
    pub const RESOURCE: u8 = 0x10;
    pub const RESOURCE_GRAPH: u8 = 0x11;
    pub const DEPENDENCY: u8 = 0x12;
    pub const ATTR_PATH: u8 = 0x13;
    pub const OPT_SOME: u8 = 0x20;
    pub const OPT_NONE: u8 = 0x21;
    pub const TYPED_STATE: u8 = 0x30;
    pub const STATE_META: u8 = 0x31;
    pub const PLAN: u8 = 0x32;
    pub const TYPED_CHANGE: u8 = 0x33;
    pub const VALUE_DIFF: u8 = 0x34;
    pub const TYPED_CONFLICT: u8 = 0x35;
    pub const PASSAPORTE: u8 = 0x36;
}

/// Emit canonical bytes for a galho IR value into a [`CanonicalSink`]. Bytes are
/// canonical-by-construction (sorted maps, length-prefixed, tag-typed).
pub trait CanonicalBytes {
    fn canonical_bytes(&self, sink: &mut CanonicalSink);
}

/// Write-only byte buffer with helpers for canonical encoding.
#[derive(Default, Debug)]
pub struct CanonicalSink {
    buf: Vec<u8>,
}

impl CanonicalSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn write_tag(&mut self, tag: u8) {
        self.buf.push(tag);
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_u32_be(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i64_be(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Write a `f64` as IEEE-754 big-endian bytes, with NaN canonicalized to a single
    /// representation (quiet NaN, payload 1). `NaN` is forbidden in `Value::Float`
    /// upstream; this is defense-in-depth.
    pub fn write_f64_be(&mut self, v: f64) {
        let bits = if v.is_nan() {
            0x7FF8_0000_0000_0001_u64
        } else {
            v.to_bits()
        };
        self.buf.extend_from_slice(&bits.to_be_bytes());
    }

    /// Write a length-prefixed byte slice (4-byte BE length + bytes).
    pub fn write_len_prefixed(&mut self, bytes: &[u8]) {
        self.write_u32_be(u32::try_from(bytes.len()).expect("len fits in u32"));
        self.buf.extend_from_slice(bytes);
    }

    /// Tag + length-prefixed bytes — the canonical "framed value" emission.
    pub fn write_tagged(&mut self, tag: u8, bytes: &[u8]) {
        self.write_tag(tag);
        self.write_len_prefixed(bytes);
    }

    pub fn write_tagged_str(&mut self, tag: u8, s: &str) {
        self.write_tagged(tag, s.as_bytes());
    }

    /// Write a sorted-key map. Keys serialize in their `BTreeMap` order (the canonical
    /// order for any `K: Ord`). Each entry: key-bytes via `write_key`, value-bytes via
    /// `write_value`.
    pub fn write_sorted_map<K, V, F, G>(
        &mut self,
        m: &BTreeMap<K, V>,
        write_key: F,
        write_value: G,
    ) where
        F: Fn(&mut CanonicalSink, &K),
        G: Fn(&mut CanonicalSink, &V),
    {
        self.write_tag(tag::MAP);
        self.write_u32_be(u32::try_from(m.len()).expect("map size fits in u32"));
        for (k, v) in m {
            write_key(self, k);
            write_value(self, v);
        }
    }

    /// Write an `Option<T>` as a tagged Some/None marker followed by `T`'s bytes when present.
    pub fn write_option<T: CanonicalBytes>(&mut self, opt: &Option<T>) {
        if let Some(v) = opt {
            self.write_tag(tag::OPT_SOME);
            v.canonical_bytes(self);
        } else {
            self.write_tag(tag::OPT_NONE);
        }
    }

    /// Append raw bytes. Used for embedding tameshi `Blake3Hash` raw bytes inside
    /// a Resource's `Applied` status.
    pub fn write_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
}

/// Convenience: emit canonical bytes from any `T: CanonicalBytes`, then hash via tameshi.
///
/// galho's IR is canonical-by-construction; we feed the emitted bytes into tameshi's
/// `RawCanonicalizer` (identity passthrough). `Strict` mode is correct because
/// re-normalizing already-canonical bytes is a no-op.
#[must_use]
pub fn content_hash<T: CanonicalBytes>(value: &T) -> Blake3Hash {
    let mut sink = CanonicalSink::new();
    value.canonical_bytes(&mut sink);
    canonical_hash(sink.as_bytes(), CanonicalMode::Strict, &RawCanonicalizer)
}

// ----- standard impls so the trait composes naturally -----

impl CanonicalBytes for str {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged_str(tag::STRING, self);
    }
}

impl CanonicalBytes for String {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged_str(tag::STRING, self.as_str());
    }
}

impl CanonicalBytes for i64 {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::INT);
        sink.write_i64_be(*self);
    }
}

impl CanonicalBytes for bool {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::BOOL);
        sink.write_u8(u8::from(*self));
    }
}

impl<T: CanonicalBytes> CanonicalBytes for Vec<T> {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tag(tag::LIST);
        sink.write_u32_be(u32::try_from(self.len()).expect("vec len fits in u32"));
        for item in self {
            item.canonical_bytes(sink);
        }
    }
}

impl<T: CanonicalBytes + ?Sized> CanonicalBytes for &T {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        (*self).canonical_bytes(sink);
    }
}

/// Raw bytes — useful for tests + cases where a typed envelope wraps opaque payload bytes.
impl CanonicalBytes for [u8] {
    fn canonical_bytes(&self, sink: &mut CanonicalSink) {
        sink.write_tagged(tag::BYTES, self);
    }
}
