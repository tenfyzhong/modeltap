#[test]
fn docker_image_builds_the_real_modeltap_sources() {
    let dockerfile = include_str!("../Dockerfile");

    assert!(!dockerfile.contains("placeholder"));
    assert!(dockerfile.contains("COPY src ./src"));
    assert!(dockerfile.contains("COPY --from=builder /app/target/release/modeltap"));
    assert!(dockerfile.contains("ENTRYPOINT [\"modeltap\"]"));
}

#[test]
fn release_workflow_publishes_macos_and_linux_binaries_for_both_supported_architectures() {
    let workflow = include_str!("../.github/workflows/release.yml");

    for target in [
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ] {
        assert!(workflow.contains(target), "missing release target {target}");
    }
    assert!(workflow.contains("actions/upload-artifact@v4"));
    assert!(workflow.contains("gh release upload"));
    assert!(workflow.contains("modeltap-${RELEASE_TAG}-"));
}
