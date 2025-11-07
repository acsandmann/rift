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
          default = {
            settings = {
              animate = true;
              animation_duration = 0.3;
              animation_fps = 100.0;
              animation_easing = "ease_in_out";

              focus_follows_mouse = true;
              mouse_follows_focus = true;
              mouse_hides_on_focus = true;

              auto_focus_blacklist = [ ];

              run_on_start = [ ];

              hot_reload = true;

              layout = {
                mode = "traditional";

                stack = {
                  stack_offset = 40.0;
                  default_orientation = "perpendicular";
                };

                gaps = {
                  outer = {
                    top = 0;
                    left = 0;
                    bottom = 0;
                    right = 0;
                  };

                  inner = {
                    horizontal = 0;
                    vertical = 0;
                  };
                };
              };

              ui = {
                menu_bar = {
                  enabled = false;
                  show_empty = false;
                };

                stack_line = {
                  enabled = false;
                  horiz_placement = "top";
                  vert_placement = "left";
                  thickness = 0.0;
                  spacing = 0.0;
                };

                mission_control = {
                  enabled = false;
                  fade_enabled = false;
                  fade_duration_ms = 180.0;
                };
              };

              gestures = {
                enabled = false;
                invert_horizontal_swipe = false;
                swipe_vertical_tolerance = 0.4;
                skip_empty = true;
                fingers = 3;
                distance_pct = 0.08;
                haptics_enabled = true;
                haptic_pattern = "level_change";
              };

              window_snapping = {
                drag_swap_fraction = 0.3;
              };
            };

            virtual_workspaces = {
              enabled = true;
              default_workspace_count = 4;
              auto_assign_windows = true;
              preserve_focus_per_workspace = true;
              workspace_auto_back_and_forth = false;

              workspace_names = [
                "first"
                "second"
              ];

              app_rules = [ ];
            };

            modifier_combinations = {
              comb1 = "Alt + Shift";
            };

            keys = {
              "Alt + Z" = "toggle_space_activated";

              "Alt + H" = {
                move_focus = "left";
              };
              "Alt + J" = {
                move_focus = "down";
              };
              "Alt + K" = {
                move_focus = "up";
              };
              "Alt + L" = {
                move_focus = "right";
              };

              "comb1 + H" = {
                move_node = "left";
              };
              "comb1 + J" = {
                move_node = "down";
              };
              "comb1 + K" = {
                move_node = "up";
              };
              "comb1 + L" = {
                move_node = "right";
              };

              "Alt + 0" = {
                switch_to_workspace = 0;
              };
              "Alt + 1" = {
                switch_to_workspace = 1;
              };
              "Alt + 2" = {
                switch_to_workspace = 2;
              };
              "Alt + 3" = {
                switch_to_workspace = 3;
              };

              "comb1 + 0" = {
                move_window_to_workspace = 0;
              };
              "comb1 + 1" = {
                move_window_to_workspace = 1;
              };
              "comb1 + 2" = {
                move_window_to_workspace = 2;
              };
              "comb1 + 3" = {
                move_window_to_workspace = 3;
              };

              "Alt + Tab" = "switch_to_last_workspace";

              "Alt + Shift + Left" = {
                join_window = "left";
              };
              "Alt + Shift + Right" = {
                join_window = "right";
              };
              "Alt + Shift + Up" = {
                join_window = "up";
              };
              "Alt + Shift + Down" = {
                join_window = "down";
              };
              "Alt + Comma" = "toggle_stack";
              "Alt + Slash" = "toggle_orientation";
              "Alt + Ctrl + E" = "unjoin_windows";

              "Alt + Shift + Space" = "toggle_window_floating";
              "Alt + F" = "toggle_fullscreen";
              "Alt + Shift + F" = "toggle_fullscreen_within_gaps";
              "comb1 + Ctrl + Space" = "toggle_focus_floating";

              "Alt + Shift + Equal" = "resize_window_grow";
              "Alt + Shift + Minus" = "resize_window_shrink";

              "Alt + Shift + D" = "debug";

              "Alt + Ctrl + S" = "serialize";
              "Alt + Ctrl + Q" = "save_and_exit";
            };
          };
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
