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

      # Binary release package with proper app bundle structure
      rift-bin = pkgs.stdenv.mkDerivation {
        pname = "rift-bin";
        version = "0.2.8";
        
        src = pkgs.fetchurl {
          url = "https://github.com/acsandmann/rift/releases/download/v0.2.8/rift-universal-macos-0.2.8.tar.gz";
          # Correct hash (the original was wrong)
          sha256 = "sha256-8urKyfNS+F3YA8X0NshQCSVnRklOy6F1lg8qGMJ6ynY=";
        };
        
        sourceRoot = ".";
        
        installPhase = ''
          # Create proper macOS app bundle structure for accessibility permissions
          mkdir -p $out/Applications/Rift.app/Contents/MacOS
          mkdir -p $out/Applications/Rift.app/Contents/Resources
          
          # Install binaries into app bundle
          cp rift $out/Applications/Rift.app/Contents/MacOS/
          cp rift-cli $out/Applications/Rift.app/Contents/MacOS/
          chmod +x $out/Applications/Rift.app/Contents/MacOS/rift
          chmod +x $out/Applications/Rift.app/Contents/MacOS/rift-cli
          
          # Create Info.plist for proper app identification
          cat > $out/Applications/Rift.app/Contents/Info.plist << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>rift</string>
  <key>CFBundleIdentifier</key>
  <string>git.acsandmann.rift</string>
  <key>CFBundleName</key>
  <string>Rift</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.2.8</string>
  <key>CFBundleVersion</key>
  <string>0.2.8</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
EOF
          
          # Also create symlinks in bin/ for CLI access
          mkdir -p $out/bin
          ln -s $out/Applications/Rift.app/Contents/MacOS/rift $out/bin/rift
          ln -s $out/Applications/Rift.app/Contents/MacOS/rift-cli $out/bin/rift-cli
        '';
        
        meta = {
          description = "Rift - A lightweight tiling window manager for macOS";
          homepage = "https://github.com/acsandmann/rift";
          platforms = lib.platforms.darwin;
          mainProgram = "rift";
        };
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
