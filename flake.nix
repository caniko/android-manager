{
  description = "Declarative Android device manager built on nix-manager-core";

  inputs = {
    rs-harbor.url = "git+https://github.com/caniko/rs-harbor.git?ref=trunk&rev=05cc4f162b55fa904b687db1821e2463fa813e50";

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";

    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";

    git-hooks.url = "github:cachix/git-hooks.nix";
    git-hooks.inputs.nixpkgs.follows = "nixpkgs";

    nix-manager-core = {
      url = "git+https://github.com/caniko/nix-manager-core?rev=ef2ed19e4747eac70daaea338e147c972e5d5f75";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rs-harbor.follows = "rs-harbor";
      inputs.rust-overlay.follows = "rust-overlay";
      inputs.crane.follows = "crane";
      inputs.treefmt-nix.follows = "treefmt-nix";
      inputs.git-hooks.follows = "git-hooks";
    };
  };

  outputs = inputs: import ./nix inputs;
}
