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
          type =
            with lib.types;
            oneOf [
              str
              path
              toml.type
              null
            ];
          description = "Configuration settings for rift. Also accepts paths (string or path type) to a config file.";
          default = ../rift.default.toml;
        };
      };

      config = lib.mkIf cfg.enable {
        launchd.user.agents.rift = {
          command = "${cfg.package}/bin/rift${
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
