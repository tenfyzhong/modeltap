use modeltap::cli::{CliError, Command, help_text, parse_arguments};
use std::io::Write;
use std::process::Command as ProcessCommand;

#[test]
fn help_flag_does_not_require_a_config_file() {
    assert_eq!(
        parse_arguments(["modeltap", "--help"]),
        Ok(Command::Help { subcommand: None })
    );
    assert_eq!(
        parse_arguments(["modeltap", "-h"]),
        Ok(Command::Help { subcommand: None })
    );
}

#[test]
fn subcommand_help_only_describes_its_own_options() {
    assert_eq!(
        parse_arguments(["modeltap", "run", "-h"]),
        Ok(Command::Help {
            subcommand: Some("run".to_owned()),
        })
    );
    assert_eq!(
        parse_arguments(["modeltap", "validate", "--help"]),
        Ok(Command::Help {
            subcommand: Some("validate".to_owned()),
        })
    );
    assert_eq!(
        parse_arguments(["modeltap", "ca-init", "help"]),
        Ok(Command::Help {
            subcommand: Some("ca-init".to_owned()),
        })
    );

    let run_help = modeltap::cli::subcommand_help_text("run").unwrap();
    assert!(run_help.contains("--config"));
    assert!(!run_help.contains("--cert"));
    assert!(!run_help.contains("SUBCOMMANDS:"));
}

#[test]
fn invalid_subcommand_arguments_report_the_specific_usage() {
    assert_eq!(
        parse_arguments(["modeltap", "run"])
            .unwrap_err()
            .to_string(),
        "usage: modeltap run --config <CONFIG>"
    );
    assert_eq!(
        parse_arguments(["modeltap", "validate", "config.yaml"])
            .unwrap_err()
            .to_string(),
        "usage: modeltap validate --config <CONFIG>"
    );
    assert_eq!(
        parse_arguments(["modeltap", "ca-init", "--cert", "ca-cert.pem"])
            .unwrap_err()
            .to_string(),
        "usage: modeltap ca-init --cert <CERTIFICATE> --key <PRIVATE_KEY>"
    );
    assert_eq!(
        CliError::Usage.to_string(),
        "usage: modeltap [COMMAND] [ARGUMENTS]; run `modeltap --help` for details"
    );
}

#[test]
fn invalid_command_output_includes_the_specific_usage() {
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_modeltap"))
        .args(["run"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "usage: modeltap run --config <CONFIG>"
    );
}

#[test]
fn run_uses_an_explicit_config_flag() {
    assert_eq!(
        parse_arguments(["modeltap", "run", "--help"]),
        Ok(Command::Help {
            subcommand: Some("run".to_owned()),
        })
    );
    assert_eq!(
        parse_arguments(["modeltap", "run", "-c", "config.yaml"]),
        Ok(Command::Run {
            config_file: "config.yaml".to_owned(),
        })
    );
    assert_eq!(
        parse_arguments(["modeltap", "run", "--config", "config.yaml"]),
        Ok(Command::Run {
            config_file: "config.yaml".to_owned(),
        })
    );
}

#[test]
fn ca_init_uses_explicit_certificate_and_private_key_flags() {
    assert_eq!(
        parse_arguments([
            "modeltap",
            "ca-init",
            "--cert",
            "ca-cert.pem",
            "--key",
            "ca-key.pem",
        ]),
        Ok(Command::CaInit {
            certificate_file: "ca-cert.pem".to_owned(),
            key_file: "ca-key.pem".to_owned(),
        })
    );
}

#[test]
fn validate_uses_the_same_config_flags_as_run() {
    for flag in ["-c", "--config"] {
        assert_eq!(
            parse_arguments(["modeltap", "validate", flag, "config.yaml"]),
            Ok(Command::Validate {
                config_file: "config.yaml".to_owned(),
            })
        );
    }
}

#[test]
fn shell_completion_files_describe_all_subcommands_and_flags() {
    let bash = include_str!("../completions/modeltap.bash");
    let zsh = include_str!("../completions/_modeltap");
    let fish = include_str!("../completions/modeltap.fish");

    for completion in [bash, zsh, fish] {
        assert!(completion.contains("ca-init"));
        assert!(completion.contains("validate"));
    }
    for completion in [bash, zsh] {
        assert!(completion.contains("--config"));
        assert!(completion.contains("--cert"));
        assert!(completion.contains("--key"));
    }
    assert!(fish.contains("-l config"));
    assert!(fish.contains("-l cert"));
    assert!(fish.contains("-l key"));
}

#[test]
fn parses_the_validate_subcommand() {
    assert_eq!(
        parse_arguments(["modeltap", "validate", "--config", "config.yaml"]),
        Ok(Command::Validate {
            config_file: "config.yaml".to_owned(),
        })
    );
    assert_eq!(
        parse_arguments(["modeltap", "validate", "--help"]),
        Ok(Command::Help {
            subcommand: Some("validate".to_owned()),
        })
    );
}

#[test]
fn validate_checks_a_file_without_starting_the_proxy() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"pricing: {timezone: UTC}\n").unwrap();

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_modeltap"))
        .args(["validate", "--config", file.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("configuration is valid:")
    );
}

#[test]
fn help_describes_commands_and_flags() {
    let help = help_text();
    assert_eq!(env!("CARGO_PKG_NAME"), "modeltap");
    assert!(help.starts_with("modeltap -"));
    assert!(help.contains("SUBCOMMANDS:"));
    assert!(help.contains("run --config <CONFIG>"));
    assert!(help.contains("validate --config <CONFIG>"));
    assert!(help.contains("ca-init --cert <CERTIFICATE> --key <PRIVATE_KEY>"));
    assert!(help.contains("-h, --help"));
    assert!(help.contains("-V, --version"));
    assert!(!help.contains("ENVIRONMENT:"));
}

#[test]
fn parses_the_version_flags() {
    assert_eq!(
        parse_arguments(["modeltap", "--version"]),
        Ok(Command::Version)
    );
    assert_eq!(parse_arguments(["modeltap", "-V"]), Ok(Command::Version));
}

#[test]
fn version_flag_prints_modeltap_with_version() {
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_modeltap"))
        .arg("--version")
        .output()
        .expect("failed to execute modeltap");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        format!("modeltap {}", modeltap::cli::VERSION)
    );

    let output_short = ProcessCommand::new(env!("CARGO_BIN_EXE_modeltap"))
        .arg("-V")
        .output()
        .expect("failed to execute modeltap");
    assert!(output_short.status.success());
    let stdout_short = String::from_utf8(output_short.stdout).unwrap();
    assert_eq!(
        stdout_short.trim(),
        format!("modeltap {}", modeltap::cli::VERSION)
    );
}
