#[test]
fn docker_image_builds_the_real_modeltap_sources() {
    let dockerfile = include_str!("../Dockerfile");

    assert!(!dockerfile.contains("placeholder"));
    assert!(dockerfile.contains("COPY src ./src"));
    assert!(dockerfile.contains("COPY benches ./benches"));
    assert!(dockerfile.contains("COPY --from=builder /app/target/release/modeltap"));
    assert!(dockerfile.contains("ENTRYPOINT [\"modeltap\"]"));
}

#[test]
fn docker_image_includes_build_script_and_git_metadata() {
    let dockerfile = include_str!("../Dockerfile");
    let dockerignore = include_str!("../.dockerignore");

    assert!(dockerfile.contains("COPY build.rs ./"));
    assert!(dockerfile.contains("COPY .git ./.git"));
    assert!(!dockerfile.contains("ARG MODELTAP_VERSION"));
    assert!(!dockerfile.contains("ENV MODELTAP_VERSION"));
    assert!(!dockerignore.lines().any(|line| line.trim() == ".git"));
}

#[test]
fn release_workflow_fetches_git_tags_without_environment_version_overrides() {
    let workflow = include_str!("../.github/workflows/release.yml");

    assert!(workflow.contains("fetch-depth: 0"));
    assert!(!workflow.contains("MODELTAP_VERSION"));
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
    assert!(workflow.contains("pull_request:"));
    assert!(workflow.contains("push:"));
    assert!(workflow.contains("- main"));
    assert!(!workflow.contains("secrets."));
    assert!(runner.contains("simulated_upstream.py"));
    assert!(runner.contains("OPENAI_COMPLETIONS_API_KEY=\"modeltap-e2e\""));
    assert!(runner.contains("OPENAI_RESPONSES_BASE_URL"));
    assert!(runner.contains("ANTHROPIC_BASE_URL"));
    assert!(runner.contains("GEMINI_BASE_URL"));
    assert!(runner.contains("opencode"));
    assert!(runner.contains("codex"));
    assert!(runner.contains("claude"));
    assert!(runner.contains("gemini"));
    assert!(runner.contains("copilot"));
    assert!(runner.contains("qwen"));
    assert!(runner.contains("simulate_agents.py"));
    assert!(runner.contains("assert_metrics.py"));

    let metrics_assertion = include_str!("../.github/e2e/assert_metrics.py");
    for agent_cli in [
        "claude_code",
        "codex",
        "oh_my_pi",
        "gemini_cli",
        "opencode",
        "pi",
        "github_copilot",
        "amazon_q",
        "roo_code",
        "qwen_code",
        "factory_droid",
        "crush",
        "kiro",
        "qoder",
        "antigravity",
        "cursor",
    ] {
        assert!(
            metrics_assertion.contains(&format!("\"{agent_cli}\"")),
            "missing expected agent_cli {agent_cli}"
        );
    }
}

#[test]
fn contributing_distinguishes_real_agent_e2e_from_simulated_protocol_regressions() {
    let contributing = include_str!("../CONTRIBUTING.md");

    for agent in [
        "Claude Code",
        "Codex",
        "oh-my-pi",
        "Gemini CLI",
        "OpenCode",
        "Pi",
        "GitHub Copilot CLI",
        "Qwen Code",
    ] {
        assert!(
            contributing.contains(&format!("| {agent} |"))
                && contributing.contains("Real E2E workflow"),
            "missing real E2E status for {agent}"
        );
    }

    for agent in [
        "Amazon Q",
        "Roo Code",
        "Factory Droid",
        "Crush",
        "Kiro",
        "Qoder",
        "Antigravity",
        "Cursor Agent",
    ] {
        assert!(
            contributing.contains(&format!("| {agent} |"))
                && contributing.contains("Simulated protocol + User-Agent regression"),
            "missing simulated status for {agent}"
        );
    }
}
