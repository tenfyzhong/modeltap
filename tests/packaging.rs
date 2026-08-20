#[test]
fn docker_image_builds_the_real_modeltap_sources() {
    let dockerfile = include_str!("../Dockerfile");

    assert!(!dockerfile.contains("placeholder"));
    assert!(dockerfile.contains("COPY src ./src"));
    assert!(dockerfile.contains("COPY --from=builder /app/target/release/modeltap"));
    assert!(dockerfile.contains("ENTRYPOINT [\"modeltap\"]"));
}
