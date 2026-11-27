{
  description = "Hulahoop — the acceptance loop for coding agents";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-linux" "x86_64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forEach = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forEach (pkgs: rec {
        hulahoop = pkgs.callPackage ./nix/package.nix { };
        default = hulahoop;
      });

      nixosModules.default = import ./nix/module.nix self;

      checks = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          vm = import ./nix/vm-test.nix { inherit pkgs; module = self.nixosModules.default; };
        });

      devShells = forEach (pkgs:
        let
          linuxInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (with pkgs; [
            webkitgtk_4_1
            gtk3
            libsoup_3
            glib
            cairo
            pango
            gdk-pixbuf
            librsvg
            at-spi2-atk
            openssl
            pkg-config
          ]);
        in {
          default = pkgs.mkShell {
            name = "hulahoop";
            packages = (with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              cargo-nextest
              nodejs_22
              pnpm
              python3
              python3Packages.pytest
              just
              jujutsu
              direnv
              nix-direnv
              sqlite
              git
              jq
            ]);
            buildInputs = linuxInputs;
            env = {
              RUST_BACKTRACE = "1";
              HULAHOOP_DEV_SHELL = "1";
            };
            shellHook = ''
              echo "hulahoop dev shell: rust $(rustc --version | cut -d' ' -f2), node $(node --version), jj $(jj --version | head -1 | cut -d' ' -f2)"
            '';
          };
        });
    };
}
