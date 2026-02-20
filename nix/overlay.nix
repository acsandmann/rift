inputs: final: prev:
let
  system = final.system;
  packageModule = (import ./package.nix inputs).perSystem {
    inherit (final) lib;
    pkgs = final;
    inherit system;
  };
in
{
  rift = packageModule.packages.rift;
  rift-bin = packageModule.packages.rift-bin;
}
