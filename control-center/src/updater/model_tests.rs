use super::*;

fn release(tag: &str, assets: serde_json::Value) -> Release {
    serde_json::from_value(
        serde_json::json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":assets}),
    )
    .unwrap()
}

#[test]
fn chooses_exact_platform_architecture_and_newer_stable_only() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let r = release(
        "v0.6.0",
        serde_json::json!([
            {"name":"lunima-team-0.6.0-win-arm64.msi","size":5,"digest":digest},
            {"name":"lunima-team-0.6.0-win-x64.msi","size":7,"digest":digest}
        ]),
    );
    assert_eq!(
        select(&r, "0.5.0", "windows", "x86_64")
            .unwrap()
            .unwrap()
            .size,
        7
    );
    assert!(select(&r, "0.6.0", "windows", "x86_64").unwrap().is_none());
    assert!(select(&r, "0.7.0", "windows", "x86_64").unwrap().is_none());
    assert!(select(&r, "0.5.0", "linux", "x86_64").is_err());
    let mut r = r;
    r.prerelease = true;
    assert!(select(&r, "0.5.0", "windows", "x86_64").unwrap().is_none());
}

#[test]
fn rejects_unverified_and_incomplete_packages_and_unsafe_versions() {
    let r = release(
        "v0.6.0",
        serde_json::json!([{"name":"lunima-team-0.6.0-win-x64.msi","size":1}]),
    );
    assert!(select(&r, "0.5.0", "windows", "x86_64").is_err());
    assert!(select(
        &release("../../evil", serde_json::json!([])),
        "0.5.0",
        "windows",
        "x86_64"
    )
    .is_err());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x");
    std::fs::write(&file, b"hello").unwrap();
    let hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    verify(&file, 5, hash).unwrap();
    assert!(verify(&file, 4, hash).is_err());
    assert!(verify(&file, 5, &"a".repeat(64)).is_err());
}

#[test]
fn pending_path_cannot_escape_the_cache_or_cross_platform() {
    let root = std::path::Path::new("cache");
    let p = Pending {
        version: "0.6.0".into(),
        name: "../evil.msi".into(),
        sha256: "a".repeat(64),
        size: 5,
    };
    assert!(p.path(root, "windows", "x86_64").is_err());
    let mut p = p;
    p.name = "lunima-team-0.6.0-win-x64.msi".into();
    assert_eq!(
        p.path(root, "windows", "x86_64").unwrap(),
        root.join("0.6.0").join(&p.name)
    );
    assert!(p.path(root, "linux", "x86_64").is_err());
}

#[test]
fn debian_packages_and_semver_order_are_selected_correctly() {
    assert_eq!(
        asset_name("0.10.0", "linux", "x86_64").unwrap(),
        "lunima-team_0.10.0_amd64.deb"
    );
    assert_eq!(
        asset_name("0.10.0", "linux", "aarch64").unwrap(),
        "lunima-team_0.10.0_arm64.deb"
    );
    assert!(newer("v0.10.0", "0.9.0").unwrap());
    assert!(!newer("v0.9.0", "0.10.0").unwrap());
    assert!(asset_name("0.6.0-beta.1", "windows", "x86_64").is_err());
    assert!(asset_name("0.6.0+../../x", "windows", "x86_64").is_err());
    assert!(asset_name("0.6.0", "linux", "arm").is_err());
}
