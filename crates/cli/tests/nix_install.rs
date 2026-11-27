//! Guards the Nix install surface: the module options, the `stable` channel
//! workflow and the README section. The runtime behaviour is covered by the
//! NixOS VM check in `nix/vm-test.nix`.

use std::fs;
use std::path::Path;

fn repo_file(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn module_declares_the_documented_options() {
    let module = repo_file("nix/module.nix");
    for option in [
        "enable",
        "package",
        "user",
        "group",
        "dataDir",
        "listen",
        "port",
        "openFirewall",
        "maxRuns",
        "harnesses",
        "piPackage",
        "environment",
    ] {
        assert!(
            module.contains(&format!("    {option} = mk")),
            "services.hulahoop.{option} is missing"
        );
    }
    assert!(module.contains("/var/lib/hulahoop"));
}

#[test]
fn flake_exports_package_module_and_vm_check() {
    let flake = repo_file("flake.nix");
    for export in ["packages", "nixosModules.default", "vm"] {
        assert!(
            flake.contains(export),
            "flake.nix does not mention {export}"
        );
    }
}

#[test]
fn release_workflow_moves_the_stable_branch() {
    let release = repo_file(".github/workflows/release.yml");
    assert!(release.contains("refs/heads/stable"));
    assert!(release.contains("v*"));
}

#[test]
fn readme_documents_channels_and_security() {
    let readme = repo_file("README.md");
    for needle in [
        "github:theskilllab/hulahoop/stable",
        "services.hulahoop",
        "connection.json",
        "Cloudflare",
        "Tailscale",
        "HUL-46",
    ] {
        assert!(readme.contains(needle), "README does not mention {needle}");
    }
}
