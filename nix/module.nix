{ self, ... }:

{
  flake.darwinModules.default =
    {
      config,
      lib,
      pkgs,
      ...
    }:
    let
      cfg = config.services.rift;

      toml = pkgs.formats.toml { };

      configFile =
        if cfg.config == null then
          null
        else if lib.isPath cfg.config || lib.isString cfg.config then
          cfg.config
        else
          toml.generate "rift.toml" cfg.config;
    in
    {
      options.services.rift = {
        enable = lib.mkEnableOption "Enable rift window manager service";

        package = lib.mkOption {
          type = lib.types.package;
          default = self.packages.${pkgs.system}.default;
          description = "rift (not rift-cli) package to use";
        };

        config = lib.mkOption {
          type = with lib.types; nullOr (oneOf [
            str
            path
            toml.type
          ]);
          description = "Configuration settings for rift. Also accepts paths (string or path type) to a config file. If null, rift uses internal defaults.";
          default = null;
        };
      };

      config = lib.mkIf cfg.enable {
        # Install rift-cli to systemPackages for CLI access
        environment.systemPackages = [ cfg.package ];

        # Custom app installation that preserves codesign identity
        # We bypass nix-darwin's automatic /Applications/Nix Apps/ copying (which re-signs)
        # and manage the installation ourselves with correct TCC identity preserved.
        system.activationScripts.applications.text = lib.mkAfter ''
          app_source="${cfg.package}/Applications/Rift.app"
          app_target="/Applications/Rift.app"
          
          echo "Installing Rift.app with preserved TCC identity..." >&2
          
          # Remove old installation if exists
          if [ -e "$app_target" ]; then
            rm -rf "$app_target"
          fi
          
          # Copy app bundle (rsync preserves codesign)
          ${pkgs.rsync}/bin/rsync -a "$app_source/" "$app_target/"
          
          # Verify codesign identity is preserved
          identity=$(/usr/bin/codesign -dv "$app_target/Contents/MacOS/rift" 2>&1 | grep "Identifier=" | cut -d= -f2)
          if [ "$identity" = "git.acsandmann.rift" ]; then
            echo "✓ TCC identity preserved: $identity" >&2
          else
            echo "⚠ TCC identity mismatch: $identity (expected git.acsandmann.rift)" >&2
            echo "  Re-signing with correct identity..." >&2
            /usr/bin/codesign --force --sign - --identifier git.acsandmann.rift "$app_target/Contents/MacOS/rift"
            /usr/bin/codesign --force --sign - --identifier git.acsandmann.rift "$app_target"
          fi
        '';

        launchd.user.agents.rift = {
          serviceConfig = {
            Label = "git.acsandmann.rift";

            # Use ProgramArguments (direct exec array) instead of nix-darwin's `command` field.
            # `command` wraps the value in `/bin/sh -c "..."`, which:
            #   1. Breaks on the space in "/Applications/Nix Apps/" (shell splits it)
            #   2. Makes macOS TCC see /bin/sh as the process instead of Rift, defeating
            #      accessibility permissions even after the user grants them.
            # ProgramArguments passes the path as an unquoted array element — no shell
            # interpretation, no space splitting, and launchd exec()s Rift directly so
            # TCC sees the correct bundle identity.
            ProgramArguments =
              [ "/Applications/Rift.app/Contents/MacOS/rift" ]
              ++ lib.optionals (configFile != null) [ "--config" (toString configFile) ];
            EnvironmentVariables = {
              RUST_LOG = "error,warn,info";
              # todo improve
              PATH = "/run/current-system/sw/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
            };
            RunAtLoad = true;
            KeepAlive = {
              SuccessfulExit = false;
              Crashed = true;
            };
            # todo add _{user} to log file name
            StandardOutPath = "/tmp/rift.out.log";
            StandardErrorPath = "/tmp/rift.err.log";
            ProcessType = "Interactive";
            LimitLoadToSessionType = "Aqua";
            Nice = -20;
          };
      };
    };
  };
}
