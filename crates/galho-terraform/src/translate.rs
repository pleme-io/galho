//! Bidirectional translation between Terraform tfstate v4 JSON and galho's canonical
//! Resource Graph IR.
//!
//! Address convention: module-prefixed resource addresses become flat IR resource ids
//! — e.g. tfstate `(module=network, type=aws_vpc, name=main)` becomes IR
//! `"module.network.aws_vpc.main"`. Multi-instance resources (count/for_each) append
//! the index key: `"aws_db_instance.replicas[0]"`.

use std::collections::{BTreeMap, BTreeSet};

use galho_types::{
    AttrPath, DependencyEdge, DepKind, GraphRoot, Provenance, Resource, ResourceGraph,
    ResourceId, ResourceKind, ResourceStatus, Value,
};
use thiserror::Error;

use crate::tfstate::{Tfstate, TfstateInstance, TfstateResource};

#[derive(Debug, Error)]
pub enum TranslateError {
    #[error("tfstate version {0} not supported (only v4)")]
    UnsupportedVersion(u32),

    #[error("invalid attribute value at {path}: {detail}")]
    InvalidAttribute { path: String, detail: String },

    #[error("invalid resource '{0}': {1}")]
    InvalidResource(String, String),
}

/// Translate a Terraform tfstate into the canonical Resource Graph IR. Each instance
/// of each resource becomes one IR resource node; the resource's dependency list
/// becomes typed `DependencyEdge`s with kind `Explicit`.
pub fn tfstate_to_canonical(state: &Tfstate) -> Result<ResourceGraph, TranslateError> {
    if state.version != 4 {
        return Err(TranslateError::UnsupportedVersion(state.version));
    }

    let root = GraphRoot {
        iac_system: "terraform".into(),
        schema_version: state.version.to_string(),
    };
    let mut resources: BTreeMap<ResourceId, Resource> = BTreeMap::new();
    let mut edges: BTreeSet<DependencyEdge> = BTreeSet::new();

    for r in &state.resources {
        for (idx, instance) in r.instances.iter().enumerate() {
            let id = build_resource_id(r, instance, idx);
            let attrs = translate_attributes(&instance.attributes)?;
            let deps: BTreeSet<ResourceId> = instance
                .dependencies
                .iter()
                .map(|d| ResourceId::new(d.clone()))
                .collect();
            for dep in &deps {
                edges.insert(DependencyEdge {
                    from: id.clone(),
                    to: dep.clone(),
                    kind: DepKind::Explicit,
                    attr_path: None,
                });
            }
            let resource = Resource {
                id: id.clone(),
                kind: ResourceKind::new(r.kind.clone()),
                attrs,
                deps,
                status: ResourceStatus::Applied {
                    generation: state.serial,
                    hash: galho_types::Blake3Hash::from([0u8; 32]),
                    applied_at: time::OffsetDateTime::now_utc(),
                },
                provenance: Provenance {
                    imported: false,
                    authored_by: Some(format!("terraform:{}", state.terraform_version)),
                    source_path: r.module.clone(),
                },
            };
            resources.insert(id, resource);
        }
    }

    Ok(ResourceGraph {
        root,
        resources,
        edges,
    })
}

/// Translate a canonical Resource Graph IR back into a Terraform tfstate. Resources
/// grouped by `(kind, base-name)` reassemble into instances per multi-instance
/// resources. The `serial` advances; `lineage` is preserved.
pub fn canonical_to_tfstate(
    graph: &ResourceGraph,
    base_state: &Tfstate,
) -> Result<Tfstate, TranslateError> {
    if graph.root.iac_system != "terraform" {
        return Err(TranslateError::InvalidResource(
            "graph.root".into(),
            format!(
                "iac_system='{}' (expected 'terraform')",
                graph.root.iac_system
            ),
        ));
    }

    // Group IR resources by (module, kind, base-name) so multi-instance resources
    // reassemble into one TfstateResource with multiple TfstateInstance entries.
    let mut grouped: BTreeMap<(Option<String>, String, String), Vec<&Resource>> =
        BTreeMap::new();
    for resource in graph.resources.values() {
        let (module, kind, name) = parse_resource_id(resource.id.as_str(), &resource.kind.0);
        grouped
            .entry((module, kind, name))
            .or_default()
            .push(resource);
    }

    let mut out = base_state.clone();
    out.bump_serial();
    out.resources = Vec::new();
    for ((module, kind, name), members) in grouped {
        let mut instances: Vec<TfstateInstance> = Vec::new();
        for member in members {
            instances.push(TfstateInstance {
                schema_version: Some(0),
                attributes: attrs_to_json(&member.attrs),
                dependencies: member.deps.iter().map(|d| d.0.clone()).collect(),
                index_key: None,
            });
        }
        out.resources.push(TfstateResource {
            mode: "managed".into(),
            kind,
            name,
            provider: format!("provider[\"registry.terraform.io/hashicorp/{}\"]", "aws"),
            instances,
            module,
        });
    }
    Ok(out)
}

// ----- helpers -----

fn build_resource_id(
    r: &TfstateResource,
    instance: &TfstateInstance,
    index: usize,
) -> ResourceId {
    let mut id = String::new();
    if let Some(module) = &r.module {
        if !module.is_empty() {
            id.push_str(module);
            id.push('.');
        }
    }
    id.push_str(&r.kind);
    id.push('.');
    id.push_str(&r.name);
    // If the instance has an index_key (count/for_each), append it.
    if let Some(key) = &instance.index_key {
        if let Some(s) = key.as_str() {
            id.push_str(&format!("[\"{s}\"]"));
        } else if let Some(n) = key.as_u64() {
            id.push_str(&format!("[{n}]"));
        }
    } else if r.instances.len() > 1 {
        id.push_str(&format!("[{index}]"));
    }
    ResourceId::new(id)
}

fn parse_resource_id(id: &str, kind_hint: &str) -> (Option<String>, String, String) {
    // Strip any trailing index ("[0]" or "[\"k\"]") for grouping.
    let stripped = id.split('[').next().unwrap_or(id);
    let parts: Vec<&str> = stripped.split('.').collect();
    if parts.len() < 2 {
        return (None, kind_hint.into(), id.into());
    }
    // Find the kind in the parts (last segment of "module.path.kind.name").
    let name = parts.last().copied().unwrap_or("").to_string();
    let kind = parts
        .get(parts.len().saturating_sub(2))
        .copied()
        .unwrap_or(kind_hint)
        .to_string();
    let module = if parts.len() > 2 {
        Some(parts[..parts.len() - 2].join("."))
    } else {
        None
    };
    (module, kind, name)
}

fn translate_attributes(
    attrs: &serde_json::Value,
) -> Result<BTreeMap<AttrPath, Value>, TranslateError> {
    let mut out = BTreeMap::new();
    if let serde_json::Value::Object(map) = attrs {
        for (key, value) in map {
            out.insert(AttrPath::new([key]), json_to_value(value));
        }
    }
    Ok(out)
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Null
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(arr) => Value::List(arr.iter().map(json_to_value).collect()),
        serde_json::Value::Object(map) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                out.insert(k.clone(), json_to_value(v));
            }
            Value::Map(out)
        }
    }
}

fn attrs_to_json(attrs: &BTreeMap<AttrPath, Value>) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (path, value) in attrs {
        let key = path.rendered();
        out.insert(key, value_to_json(value));
    }
    serde_json::Value::Object(out)
}

fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Bytes(b) => {
            // Emit bytes as base64-encoded string; this is the conventional
            // terraform-binary-attribute carriage.
            use std::fmt::Write;
            let mut hex = String::with_capacity(b.len() * 2);
            for byte in b {
                let _ = write!(&mut hex, "{byte:02x}");
            }
            serde_json::Value::String(hex)
        }
        Value::List(items) => {
            serde_json::Value::Array(items.iter().map(value_to_json).collect())
        }
        Value::Map(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                out.insert(k.clone(), value_to_json(v));
            }
            serde_json::Value::Object(out)
        }
        Value::SecretRef(r) => {
            // SecretRefs MUST NOT have their resolved values in tfstate. Emit a
            // typed marker that the operator + auditor can grep for.
            let mut obj = serde_json::Map::new();
            obj.insert(
                "_galho_secret_ref".into(),
                serde_json::Value::String(format!(
                    "{}://{}@{}",
                    r.backend,
                    r.path,
                    r.version.as_deref().unwrap_or("latest"),
                )),
            );
            serde_json::Value::Object(obj)
        }
        Value::CrossSystemRef(r) => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "_galho_cross_system_ref".into(),
                serde_json::Value::String(format!(
                    "{}/{} -> {}/{}",
                    r.from_system, r.from_resource, r.to_system, r.to_resource,
                )),
            );
            serde_json::Value::Object(obj)
        }
    }
}
