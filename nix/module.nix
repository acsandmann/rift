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
        # Add rift to systemPackages for stable /Applications/Nix Apps/ installation
        # This creates a stable directory (not symlink) that preserves TCC permissions across rebuilds
        environment.systemPackages = [ cfg.package ];

        launchd.user.agents.rift = {
          # Launch via the stable /Applications/Nix Apps/ path, NOT the Nix store path.
          # macOS TCC (accessibility permissions) is tied to the app bundle path. nix-darwin
          # copies app bundles from the Nix store to /Applications/Nix Apps/ as a stable
          # non-symlink directory — that's the path the user grants accessibility to.
          # Using the Nix store path directly (${cfg.package}/bin/rift) bypasses this, causing
          # Rift to report "Accessibility permission is not granted" on every launch even
          # after the user has granted it in System Settings.
          command = "/Applications/Nix Apps/Rift.app/Contents/MacOS/rift${
            if configFile == null then "" else " --config " + lib.escapeShellArg configFile
          }";

          serviceConfig = {
            Label = "git.acsandmann.rift";
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
