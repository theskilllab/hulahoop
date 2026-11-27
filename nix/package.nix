# The headless binary: server, runner and cli in one executable. Migrations are
# embedded and no sqlx query is checked at compile time, so no database is needed
# (sqlite is only linked in, via libsqlite3-sys).
{ lib, rustPlatform, makeWrapper, pkg-config, sqlite, jujutsu, git, direnv, nix-direnv }:

rustPlatform.buildRustPackage {
  pname = "hulahoop";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "-p" "hulahoop-cli" ];
  # The suite is `just test`; it needs git, jj and a writable HOME.
  doCheck = false;

  nativeBuildInputs = [ makeWrapper pkg-config ];
  buildInputs = [ sqlite ];

  # The runner calls these directly, so they ride on the binary's PATH.
  postInstall = ''
    wrapProgram $out/bin/hulahoop \
      --prefix PATH : ${lib.makeBinPath [ jujutsu git direnv nix-direnv ]}
  '';

  meta = {
    description = "The acceptance loop for coding agents (server, runner, cli)";
    homepage = "https://github.com/theskilllab/hulahoop";
    license = lib.licenses.mit;
    mainProgram = "hulahoop";
    platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
  };
}
