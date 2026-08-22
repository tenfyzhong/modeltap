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

#[test]
fn release_workflow_normalizes_homebrew_bottle_asset_names() {
    let workflow = include_str!("../.github/workflows/release.yml");

    assert!(workflow.contains("modeltap--*.bottle.tar.gz"));
    assert!(workflow.contains("${bottle/modeltap--/modeltap-}"));
}

#[test]
fn agent_e2e_workflow_exercises_each_supported_protocol_and_agent() {
    let workflow = include_str!("../.github/workflows/agent-e2e.yml");
    let runner = include_str!("../.github/e2e/run-agent-e2e.sh");

    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains("OPENAI_COMPLETIONS_API_KEY"));
    assert!(workflow.contains("OPENAI_RESPONSES_API_KEY"));
    assert!(workflow.contains("ANTHROPIC_API_KEY"));
    assert!(workflow.contains("GEMINI_API_KEY"));
    assert!(workflow.contains("OPENAI_COMPLETIONS_BASE_URL"));
    assert!(workflow.contains("OPENAI_RESPONSES_BASE_URL"));
    assert!(workflow.contains("ANTHROPIC_BASE_URL"));
    assert!(workflow.contains("GEMINI_BASE_URL"));
    assert!(runner.contains("opencode"));
    assert!(runner.contains("codex"));
    assert!(runner.contains("claude"));
    assert!(runner.contains("gemini"));
    assert!(runner.contains("assert_metrics.py"));
}
