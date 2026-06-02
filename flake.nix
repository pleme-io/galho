{
  description = "galho — branch-aware typed IaC state. Canonical Resource Graph IR + content-addressed Merkle DAG + three-way merge algebra. IaC-system-agnostic; terraform first-class via galho-magma. Consumes tameshi::Canonicalizer + engenho-controllers::Controller as canonical second consumers. Spec: theory/GALHO.md.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    crate2nix.url = "github:nix-community/crate2nix";
    flake-utils.url = "github:numtide/flake-utils";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crate2nix, flake-utils, substrate, ... }:
    (import "${substrate}/lib/rust-workspace-release-flake.nix" {
      inherit nixpkgs crate2nix flake-utils;
    }) {
      toolName = "galho";
      packageName = "galho-cli";
      src = self;
      repo = "pleme-io/galho";
    };
}
