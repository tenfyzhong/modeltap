#[test]
fn docker_image_builds_the_real_modeltap_sources() {
    let dockerfile = include_str!("../Dockerfile");

    assert!(!dockerfile.contains("placeholder"));
    assert!(dockerfile.contains("COPY src ./src"));
    assert!(dockerfile.contains("COPY --from=builder /app/target/release/modeltap"));
    assert!(dockerfile.contains("ENTRYPOINT [\"modeltap\"]"));
}

#[test]
fn compose_uses_a_user_provided_config_yaml_file() {
    let compose = include_str!("../docker-compose.yml");

    assert!(compose.contains("./config.yaml:/etc/modeltap/config.yaml:ro"));
    assert!(!compose.contains("config.docker.yaml"));
}

#[test]
fn release_workflow_excludes_intel_macos_binaries() {
    let workflow = include_str!("../.github/workflows/release.yml");

    for target in [
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ] {
        assert!(workflow.contains(target), "missing release target {target}");
    }
    assert!(!workflow.contains("x86_64-apple-darwin"));
    assert!(!workflow.contains("macos-13"));
    assert!(workflow.contains("actions/upload-artifact@v4"));
    assert!(workflow.contains("gh release upload"));
    assert!(workflow.contains("modeltap-${RELEASE_TAG}-"));
}
