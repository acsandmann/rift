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

      # Wrap built binaries in proper macOS app bundle for TCC permissions
      rift-app = pkgs.stdenv.mkDerivation {
        pname = "rift";
        version = "0.1.0";
        
        buildInputs = [ build ];
        
        dontUnpack = true;
        
        # codesign needs to touch the Mach-O binary outside the sandbox
        __noChroot = true;
        
        installPhase = ''
          # Create proper macOS app bundle structure for accessibility permissions
          mkdir -p $out/Applications/Rift.app/Contents/MacOS
          mkdir -p $out/Applications/Rift.app/Contents/Resources
          
          # Install binaries from crane build into app bundle
          cp ${build}/bin/rift $out/Applications/Rift.app/Contents/MacOS/
          chmod +x $out/Applications/Rift.app/Contents/MacOS/rift
          
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
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
EOF
          
          # Ad-hoc codesign with explicit bundle identifier so TCC matches git.acsandmann.rift
          # --identifier overrides the default (binary filename) and binds CFBundleIdentifier into
          # the codesign identity, ensuring TCC lookups find the correct entry.
          /usr/bin/codesign --force --sign - \
            --identifier git.acsandmann.rift \
            $out/Applications/Rift.app/Contents/MacOS/rift
          /usr/bin/codesign --force --sign - \
            --identifier git.acsandmann.rift \
            $out/Applications/Rift.app
          
          # Also create symlinks in bin/ for CLI access
          mkdir -p $out/bin
          ln -s $out/Applications/Rift.app/Contents/MacOS/rift $out/bin/rift
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
      # Build outputs
      checks.rift = build;  # Ensure build succeeds in CI

      packages.rift = rift-app;  # Main package with app bundle
      packages.rift-unwrapped = build;  # Raw binaries for development
      packages.default = rift-app;

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
