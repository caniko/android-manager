{
  description = "Declarative Android device manager built on nix-manager-core";

  inputs = {
    harbor.url = "git+https://github.com/caniko/harbor.git?ref=feat/harbor-monorepo-components&rev=e8b1996580e4ecd51dd402edf4a16d9df88e3a1b";

    nixpkgs.follows = "harbor/nixpkgs";
    rust-overlay.follows = "harbor/rust-overlay";
    crane.follows = "harbor/crane";

    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";

    git-hooks.url = "github:cachix/git-hooks.nix";
    # git-hooks evaluates its own package set on Intel macOS.
    git-hooks.inputs.nixpkgs.follows = "harbor/nixpkgs-darwin";

    nix-manager-core = {
      url = "git+https://github.com/caniko/nix-manager-core.git?ref=migrate/harbor-combined-20261012&rev=42b11a63a955e9ab0793bb641daf656747f71dea";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.harbor.follows = "harbor";
      inputs.rust-overlay.follows = "rust-overlay";
      inputs.crane.follows = "crane";
      inputs.treefmt-nix.follows = "treefmt-nix";
      inputs.git-hooks.follows = "git-hooks";
    };
  };

  outputs = inputs: import ./nix inputs;
}
