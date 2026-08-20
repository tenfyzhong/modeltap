use modeltap::cli::{Command, help_text, parse_arguments};

#[test]
fn help_flag_does_not_require_a_config_file() {
    assert_eq!(parse_arguments(["modeltap", "--help"]), Ok(Command::Help));
    assert_eq!(parse_arguments(["modeltap", "-h"]), Ok(Command::Help));
}

#[test]
fn run_help_does_not_require_a_config_file() {
    assert_eq!(
        parse_arguments(["modeltap", "run", "--help"]),
        Ok(Command::Help)
    );
    assert_eq!(
        parse_arguments(["modeltap", "run", "config.yaml"]),
        Ok(Command::Run {
            config_file: "config.yaml".to_owned(),
        })
    );
}

#[test]
fn help_describes_commands_and_flags() {
    let help = help_text();
    assert_eq!(env!("CARGO_PKG_NAME"), "modeltap");
    assert!(help.starts_with("modeltap -"));
    assert!(help.contains("SUBCOMMANDS:"));
    assert!(help.contains("run <CONFIG>"));
    assert!(help.contains("ca-init <CERTIFICATE> <PRIVATE_KEY>"));
    assert!(help.contains("-h, --help"));
    assert!(help.contains("-V, --version"));
    assert!(help.contains("logging.level: debug"));
}
