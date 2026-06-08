{
  pkgs,
  crane,
  rev ? "dirty",
}:
let
  lib = pkgs.lib;
  craneLib = crane.mkLib pkgs;
  cargoToml = lib.importTOML ./Cargo.toml;
  src = lib.cleanSourceWith {
    src = ./.; # The original, unfiltered source
    # TODO(DRY): Consolidate with that of default-crates.nix
    filter =
      path: type:
      lib.hasSuffix ".plist" path
      ||
        # Default filter from crane (allow .rs files)
        (craneLib.filterCargoSources path type);
  };

  commonArgs = {
    inherit src;
    pname = "rift";
    version = "${cargoToml.package.version}-${rev}";
    strictDeps = true;
    cargoExtraArgs = "--workspace";
    buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
    nativeCheckInputs = lib.optionals (!pkgs.stdenv.hostPlatform.isDarwin) [ pkgs.sudo ];
  };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;

    nativeBuildInputs = [
      pkgs.installShellFiles
      pkgs.makeBinaryWrapper
    ];

    postInstall = ''
      # Generate shell completions and the man page using the installed xtask.
      $out/bin/xtask dist

      # Remove xtask from the final output; it is only needed during install.
      rm $out/bin/xtask

      installShellCompletion --cmd rift ./comp/rift.{bash,fish,zsh,nu}
      installShellCompletion --cmd rift-cli ./comp/rift-cli.{bash,fish,zsh,nu}

      installManPage ./man/rift.1
      installManPage ./man/rift-cli.1
    '';

    nativeInstallCheckInputs = [ pkgs.versionCheckHook ];
    doInstallCheck = false;
    versionCheckProgram = "${placeholder "out"}/bin/rift";
    versionCheckProgramArg = "--version";

    meta = {
      description = "Rift Window manager";
      homepage = "https://github.com/acsandmann/rift";
      license = lib.licenses.eupl12;
      mainProgram = "rift";
      maintainers = with lib.maintainers; [
        auscyber
      ];
    };
  }
)
