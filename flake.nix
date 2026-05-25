{
  description = "Description for the project";

  inputs = {
    nixpkgs.url = "github:cachix/devenv-nixpkgs/rolling";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
    rust-flake.url = "github:juspay/rust-flake";

    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    nix2container.url = "github:nlewo/nix2container";
    nix2container.inputs.nixpkgs.follows = "nixpkgs";
    mk-shell-bin.url = "github:rrbutani/nix-mk-shell-bin";
  };

  nixConfig = {
    extra-trusted-public-keys = "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw=";
    extra-substituters = "https://devenv.cachix.org";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.rust-flake.flakeModules.default
        inputs.rust-flake.flakeModules.nixpkgs
        inputs.flake-parts.flakeModules.easyOverlay
      ];
      systems = [
        "x86_64-linux"
        "i686-linux"
        "x86_64-darwin"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      perSystem =
        {
          config,
          self',
          inputs',
          pkgs,
          lib,
          system,
          ...
        }:
        {
          overlayAttrs = {
            rift = config.packages.rift-wm;
          };
          # Per-system attributes can be defined here. The self' and inputs'
          rust-project.src = lib.cleanSourceWith {
            src = inputs.self; # The original, unfiltered source
            # TODO(DRY): Consolidate with that of default-crates.nix
            filter =
              path: type:
              (
                config.rust-project.crateNixFile != null
                && lib.hasSuffix "/${config.rust-project.crateNixFile}" path
              )
              || lib.hasSuffix ".plist" path
              ||
                # Default filter from crane (allow .rs files)
                (config.rust-project.crane-lib.filterCargoSources path type);
          };
          # module parameters provide easy access to attributes of the same
          # system.

          # Equivalent to  inputs'.nixpkgs.legacyPackages.hello;

          treefmt.config = {

          };
          devShells.default = config.devShells.rust;
        };
      flake =
        { config, ... }:
        {
          # The usual flake attributes can be defined here, including system-
          # agnostic ones like nixosModule and system-enumerating ones, although
          # those are more easily expressed in perSystem.

        };
      debug = true;
    };
}
