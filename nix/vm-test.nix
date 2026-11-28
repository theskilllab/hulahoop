# Boots a NixOS machine with services.hulahoop and checks the token gate and the runner.
{ pkgs, module }:

pkgs.testers.runNixOSTest {
  name = "hulahoop";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    # The test script calls these; `which` makes the absence checks below meaningful.
    environment.systemPackages = [ pkgs.curl pkgs.which ];
    services.hulahoop.enable = true;
    # Agents enter dev shells through the daemon, which the VM has by default.
  };

  testScript = { nodes, ... }: ''
    import json

    machine.wait_for_unit("hulahoop.service")
    machine.wait_for_file("/var/lib/hulahoop/connection.json")
    machine.wait_for_open_port(9000)

    def read_token():
        return json.loads(machine.succeed("cat /var/lib/hulahoop/connection.json"))["token"]

    url = "http://127.0.0.1:9000/graphql"
    body = "-H 'content-type: application/json' -d '{\"query\":\"{ runnerCapacity { reportedAt } }\"}'"

    # The token goes in through a config on stdin, not onto the command line.
    def curl(token, fmt):
        header = f"header = \"authorization: Bearer {token}\"" if token else ""
        return f"echo '{header}' | curl -s -K - -o /dev/null -w '{fmt}' {body} {url}"

    def status(token):
        return machine.succeed(curl(token, "%{http_code}")).strip()

    token = read_token()
    assert status(None) == "401"
    assert status(token) == "200"

    # The runner registers by reporting its capacity; null until it has.
    machine.wait_until_succeeds(
        f"echo 'header = \"authorization: Bearer {token}\"' | curl -s -K - {body} {url} | grep -q reportedAt"
    )

    # The data directory belongs to the service user.
    assert machine.succeed("stat -c %U:%G /var/lib/hulahoop").strip() == "hulahoop:hulahoop"

    # The tools the runner calls come with the package, not the system.
    # (direnv is installed system-wide by programs.direnv, so it is not checked here.)
    for tool in ["jj", "git"]:
        machine.fail(f"which {tool}")
    # nix-direnv is wired in through direnv's config, which caches dev shells.
    machine.succeed("grep -q nix-direnv /etc/direnv/direnvrc")
    pid = machine.succeed("systemctl show -p MainPID --value hulahoop.service").strip()
    service_path = machine.succeed(f"tr '\\0' '\\n' < /proc/{pid}/environ | grep '^PATH='")
    assert "DIRENV_CONFIG=/etc/direnv" in machine.succeed(f"tr '\\0' '\\n' < /proc/{pid}/environ")
    for tool in ["jujutsu", "git", "direnv"]:
        assert tool in service_path, f"{tool} missing from the service PATH: {service_path}"
    # pi, claude and just are absent here, so doctor exits non-zero; read its lines.
    doctor = machine.succeed("${nodes.machine.services.hulahoop.package}/bin/hulahoop doctor || true")
    for tool in ["jj", "direnv", "nix"]:
        assert f"ok    {tool}" in doctor, doctor

    # A restart keeps the data, and with it the same token.
    machine.succeed("systemctl restart hulahoop.service")
    machine.wait_for_open_port(9000)
    machine.wait_for_file("/var/lib/hulahoop/connection.json")
    assert read_token() == token
    assert status(token) == "200"
  '';
}
