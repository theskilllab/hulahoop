# `services.hulahoop`: the server and the runner as one systemd service.
self:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.hulahoop;
  inherit (lib) mkOption mkEnableOption mkIf mkMerge types;
in
{
  options.services.hulahoop = {
    enable = mkEnableOption "the Hulahoop server and runner";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.hulahoop;
      defaultText = lib.literalExpression "hulahoop.packages.\${system}.hulahoop";
      description = "The hulahoop package to run.";
    };

    user = mkOption {
      type = types.str;
      default = "hulahoop";
      description = "User the service runs as; created by this module when left at the default.";
    };

    group = mkOption {
      type = types.str;
      default = "hulahoop";
      description = "Group the service runs as; created by this module when left at the default, even if `user` is customised.";
    };

    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/hulahoop";
      description = "Where the database, vault key, mirrors, checkouts and connection.json live.";
    };

    listen = mkOption {
      type = types.str;
      default = "127.0.0.1";
      description = ''
        IP address (a literal, not a hostname) to listen on. The app sends connector credentials over plain HTTP
        with one bearer token, so keep this on loopback or a private interface
        (Tailscale) or put a TLS reverse proxy in front.
      '';
    };

    port = mkOption {
      type = types.port;
      default = 9000;
      description = "TCP port to listen on.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open `port` in the firewall. Leave off unless the network is trusted.";
    };

    maxRuns = mkOption {
      type = types.nullOr types.ints.positive;
      default = null;
      description = "Most runs that share the host (HULAHOOP_MAX_RUNS). Null keeps the built-in default.";
    };

    harnesses = mkOption {
      type = types.listOf types.package;
      default = [ ];
      description = "Agent CLIs put on the runner's PATH. Add `pkgs.just` too if a repository's dev shell does not provide it.";
    };

    piPackage = mkOption {
      type = types.nullOr types.package;
      default = null;
      description = "Package providing `pi`; sets HULAHOOP_PI_BIN to its main program.";
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      description = ''
        Extra environment variables for the service. They are written to the unit file in the
        world-readable Nix store, so do not put secrets here.
      '';
    };
  };

  config = mkIf cfg.enable (mkMerge [
    {
      users.users = mkIf (cfg.user == "hulahoop") {
        hulahoop = {
          isSystemUser = true;
          group = cfg.group;
          home = cfg.dataDir;
        };
      };
      # direnv finds nix-direnv through its config (DIRENV_CONFIG=/etc/direnv), not
      # PATH; without it every `direnv exec` re-evaluates the repo's flake.
      programs.direnv.enable = lib.mkDefault true;
      programs.direnv.nix-direnv.enable = lib.mkDefault true;

      users.groups = mkIf (cfg.group == "hulahoop") { hulahoop = { }; };

      systemd.tmpfiles.rules = [ "d ${cfg.dataDir} 0700 ${cfg.user} ${cfg.group} -"
        # `d` leaves an existing directory alone, so re-own it if user/group change.
        "z ${cfg.dataDir} 0700 ${cfg.user} ${cfg.group} -"
      ];

      systemd.services.hulahoop = {
        description = "Hulahoop server and runner";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" "nix-daemon.socket" ];
        wants = [ "nix-daemon.socket" ];
        # The runner enters each repo's dev shell through direnv and nix, and
        # agents run the harnesses by name.
        path = [ config.nix.package ] ++ cfg.harnesses;
        environment = {
          HOME = cfg.dataDir;
          NIX_REMOTE = "daemon";
        }
        // lib.optionalAttrs (cfg.maxRuns != null) { HULAHOOP_MAX_RUNS = toString cfg.maxRuns; }
        // lib.optionalAttrs (cfg.piPackage != null) {
          HULAHOOP_PI_BIN = lib.getExe cfg.piPackage;
        }
        // cfg.environment;
        serviceConfig = {
          ExecStart = "${lib.getExe cfg.package} up --listen ${lib.escapeShellArg cfg.listen} --port ${toString cfg.port} --data-dir ${lib.escapeShellArg cfg.dataDir}";
          User = cfg.user;
          Group = cfg.group;
          WorkingDirectory = cfg.dataDir;
          Restart = "on-failure";
          RestartSec = 5;
        };
      };
    }
    (mkIf cfg.openFirewall {
      networking.firewall.allowedTCPPorts = [ cfg.port ];
    })
  ]);
}
