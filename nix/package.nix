{
  crane,
  fenix,
  ...
}:
{
  perSystem =
    {
      pkgs,
      lib,
      system,
      ...
    }:
    let
      toolchain = fenix.packages.${system}.stable.toolchain;
      craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
      root = ../.;

      args = {
        src = lib.fileset.toSource {
          inherit root;
          fileset = lib.fileset.unions [
            (craneLib.fileset.commonCargoSources root)
            (lib.fileset.fileFilter (file: file.hasExt "plist") root)
          ];
        };
        strictDeps = true;
        doCheck = false;

        nativeBuildInputs = [ ];
        buildInputs = [ ];
      };

      build = craneLib.buildPackage (
        args
        // {
          cargoArtifacts = craneLib.buildDepsOnly args;
        }
      );

      rift-bin = pkgs.stdenv.mkDerivation {
        pname = "rift-bin";
        version = "0.2.8";
        src = builtins.fetchTarball {
          url = "https://github.com/acsandmann/rift/releases/download/v0.2.8/rift-universal-macos-0.2.8.tar.gz";
          sha256 = "1cm3nqz6bl01i337yg1l9v616w4kkcsc1m725s9hgj5zgprhybna";
        };
        phases = [ "installPhase" ];
        installPhase = ''
          mkdir -p $out/bin
          cp -r $src/* $out/bin
          chmod +x $out/bin/*
        '';
      };
    in
    {
      checks.rift = build;

      packages.rift = build;
      packages.rift-bin = rift-bin;
      packages.default = rift-bin;

      devshells.default = {
        packages = [
          toolchain
        ];
        commands = [
          {
            help = "";
            name = "hot";
            command = "${pkgs.watchexec}/bin/watchexec -e rs -w src -w Cargo.toml -w Cargo.lock -r ${toolchain}/bin/cargo run -- $@";
          }
        ];
      };
    };
}
