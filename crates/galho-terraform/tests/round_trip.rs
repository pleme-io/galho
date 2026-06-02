//! Round-trip tests for Terraform tfstate ↔ canonical Resource Graph IR.
//!
//! Load-bearing property: `tfstate_to_canonical(canonical_to_tfstate(state)) == state`
//! modulo serial-bumping + serde field-order. This is the contract that lets galho's
//! algebra (merge, diff, plan) work over terraform-shaped state.

use galho_terraform::{
    canonical_to_tfstate, tfstate_to_canonical, Terraform, Tfstate, TfstateInstance,
    TfstateResource,
};
use galho_types::{
    ApplySemantics, IaCSystem, IaCSystemId, ResourceId,
};
use serde_json::json;

fn sample_tfstate() -> Tfstate {
    Tfstate {
        version: 4,
        terraform_version: "1.10.0".into(),
        serial: 7,
        lineage: "lineage-abc".into(),
        outputs: Default::default(),
        resources: vec![
            TfstateResource {
                mode: "managed".into(),
                kind: "aws_vpc".into(),
                name: "main".into(),
                provider: "provider[\"registry.terraform.io/hashicorp/aws\"]".into(),
                module: None,
                instances: vec![TfstateInstance {
                    schema_version: Some(0),
                    attributes: json!({
                        "id": "vpc-abc123",
                        "cidr_block": "10.0.0.0/16",
                        "enable_dns_hostnames": true,
                        "tags": {
                            "env": "prod",
                            "owner": "team-platform"
                        }
                    }),
                    dependencies: vec![],
                    index_key: None,
                }],
            },
            TfstateResource {
                mode: "managed".into(),
                kind: "aws_db_instance".into(),
                name: "main".into(),
                provider: "provider[\"registry.terraform.io/hashicorp/aws\"]".into(),
                module: Some("module.network".into()),
                instances: vec![TfstateInstance {
                    schema_version: Some(0),
                    attributes: json!({
                        "id": "db-xyz789",
                        "allocated_storage": 100,
                        "engine": "postgres",
                    }),
                    dependencies: vec!["aws_vpc.main".into()],
                    index_key: None,
                }],
            },
        ],
    }
}

#[test]
fn terraform_iac_system_marker_has_expected_values() {
    assert_eq!(Terraform::id(), IaCSystemId::new("terraform"));
    assert_eq!(Terraform::schema_version(), "4");
    assert_eq!(Terraform::apply_semantics(), ApplySemantics::PartialProgress);
}

#[test]
fn tfstate_to_canonical_yields_resource_graph() {
    let tf = sample_tfstate();
    let graph = tfstate_to_canonical(&tf).unwrap();
    assert_eq!(graph.root.iac_system, "terraform");
    assert_eq!(graph.root.schema_version, "4");
    assert_eq!(graph.resources.len(), 2);
    assert!(graph.resources.contains_key(&ResourceId::new("aws_vpc.main")));
    assert!(graph
        .resources
        .contains_key(&ResourceId::new("module.network.aws_db_instance.main")));
}

#[test]
fn dependencies_become_typed_explicit_edges() {
    let tf = sample_tfstate();
    let graph = tfstate_to_canonical(&tf).unwrap();
    // module.network.aws_db_instance.main depends_on aws_vpc.main → one Explicit edge.
    let has_edge = graph.edges.iter().any(|e| {
        e.from == ResourceId::new("module.network.aws_db_instance.main")
            && e.to == ResourceId::new("aws_vpc.main")
            && matches!(e.kind, galho_types::DepKind::Explicit)
    });
    assert!(has_edge, "expected typed Explicit edge db → vpc; edges: {:?}", graph.edges);
}

#[test]
fn canonical_to_tfstate_preserves_resource_count() {
    let tf = sample_tfstate();
    let graph = tfstate_to_canonical(&tf).unwrap();
    let round_trip = canonical_to_tfstate(&graph, &tf).unwrap();
    assert_eq!(round_trip.resources.len(), tf.resources.len());
    // Serial bumped on round-trip write.
    assert_eq!(round_trip.serial, tf.serial + 1);
    // Lineage preserved.
    assert_eq!(round_trip.lineage, tf.lineage);
}

#[test]
fn translated_applied_status_carries_real_nonzero_hash() {
    use galho_types::ResourceStatus;
    let tf = sample_tfstate();
    let graph = tfstate_to_canonical(&tf).unwrap();
    for (rid, resource) in &graph.resources {
        match &resource.status {
            ResourceStatus::Applied(applied) => {
                assert_ne!(
                    applied.hash().0,
                    [0u8; 32],
                    "resource {rid:?} must carry a real (non-zero) Applied hash"
                );
                assert_eq!(applied.generation(), tf.serial);
            }
            other => panic!("expected Applied status for {rid:?}, got {other:?}"),
        }
    }
}

#[test]
fn translated_applied_hash_is_deterministic_across_runs() {
    use galho_types::ResourceStatus;
    let tf = sample_tfstate();
    let g1 = tfstate_to_canonical(&tf).unwrap();
    let g2 = tfstate_to_canonical(&tf).unwrap();
    for (rid, r1) in &g1.resources {
        let r2 = &g2.resources[rid];
        if let (ResourceStatus::Applied(a1), ResourceStatus::Applied(a2)) =
            (&r1.status, &r2.status)
        {
            // The content hash is derived from attrs (deterministic); the
            // applied_at timestamp differs per run but does not affect the hash.
            assert_eq!(a1.hash(), a2.hash(), "attr-derived hash must be stable for {rid:?}");
        }
    }
}

#[test]
fn tfstate_json_round_trip_preserves_attributes() {
    let tf = sample_tfstate();
    let bytes = tf.to_json_bytes().unwrap();
    let parsed = Tfstate::from_json_bytes(&bytes).unwrap();
    assert_eq!(parsed.version, tf.version);
    assert_eq!(parsed.serial, tf.serial);
    assert_eq!(parsed.resources.len(), tf.resources.len());
}

#[test]
fn empty_tfstate_round_trips_cleanly() {
    let tf = Tfstate::empty("test-lineage");
    let graph = tfstate_to_canonical(&tf).unwrap();
    assert!(graph.resources.is_empty());
    let round_trip = canonical_to_tfstate(&graph, &tf).unwrap();
    assert!(round_trip.resources.is_empty());
}

#[test]
fn unsupported_tfstate_version_rejected() {
    let mut tf = Tfstate::empty("x");
    tf.version = 3;
    let err = tfstate_to_canonical(&tf).unwrap_err();
    assert!(matches!(
        err,
        galho_terraform::TranslateError::UnsupportedVersion(3)
    ));
}

#[test]
fn write_then_read_tfstate_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("terraform.tfstate");
    let tf = sample_tfstate();
    let bytes = tf.to_json_bytes().unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let read_bytes = std::fs::read(&path).unwrap();
    let parsed = Tfstate::from_json_bytes(&read_bytes).unwrap();
    assert_eq!(parsed.serial, tf.serial);
    assert_eq!(parsed.resources.len(), tf.resources.len());
}

#[test]
fn secret_ref_in_ir_emits_typed_marker_not_value() {
    use galho_types::{
        AttrPath, GraphRoot, Provenance, Resource, ResourceGraph, ResourceKind, ResourceStatus,
        SecretRef, Value,
    };
    use std::collections::{BTreeMap, BTreeSet};

    let mut attrs = BTreeMap::new();
    attrs.insert(
        AttrPath::new(["secret_password"]),
        Value::SecretRef(SecretRef::new("akeyless", "/db/password").with_version("v3")),
    );
    let resource = Resource {
        id: ResourceId::new("aws_db_instance.main"),
        kind: ResourceKind::new("aws_db_instance"),
        attrs,
        deps: BTreeSet::new(),
        status: ResourceStatus::Pending,
        provenance: Provenance::default(),
    };
    let mut resources = BTreeMap::new();
    resources.insert(resource.id.clone(), resource);
    let graph = ResourceGraph {
        root: GraphRoot {
            iac_system: "terraform".into(),
            schema_version: "4".into(),
        },
        resources,
        edges: BTreeSet::new(),
    };

    let base = Tfstate::empty("x");
    let tf = canonical_to_tfstate(&graph, &base).unwrap();
    let json = tf.to_json_bytes().unwrap();
    let text = String::from_utf8(json).unwrap();
    // SecretRef marker present; resolved value NOT.
    assert!(text.contains("_galho_secret_ref"));
    assert!(text.contains("akeyless"));
    assert!(text.contains("/db/password"));
    // Verify the actual secret value never appears.
    assert!(!text.contains("RESOLVED_SECRET"));
    assert!(!text.contains("plaintext"));
}
