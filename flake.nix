{
  description = "galho — branch-aware typed IaC state. Canonical Resource Graph IR + content-addressed Merkle DAG + three-way merge algebra. IaC-system-agnostic; terraform first-class via galho-magma. Consumes tameshi::Canonicalizer + engenho-controllers::Controller as canonical second consumers. Spec: theory/GALHO.md.";

  inputs = {
    nixpkgs = {
      url = "github:nixos/nixpkgs?ref=nixos-unstable";
    };
    flake-utils = {
      url = "github:numtide/flake-utils";
    };
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ { self, nixpkgs, crate2nix, fenix, substrate, ... }:
    (import "${substrate}/lib/rust-library-workspace-flake.nix" {
      inherit nixpkgs crate2nix fenix;
    }) {
      workspaceName = "galho";
      members = [
        "galho-types"
        "galho-storage"
      ];
      defaultMember = "galho-types";
      src = self;
    };
}
