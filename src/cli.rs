use thiserror::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn help_text() -> &'static str {
    "modeltap - AI API MITM proxy and usage monitor

USAGE:
  modeltap [COMMAND] [ARGUMENTS]

SUBCOMMANDS:
  run --config <CONFIG>                Start the proxy with a YAML configuration file.
  validate --config <CONFIG>           Validate a YAML configuration file without starting the proxy.
  ca-init --cert <CERTIFICATE> --key <PRIVATE_KEY>
                                       Create a new local root CA certificate and private key.
  help                                 Print this help text.

GLOBAL FLAGS:
  -h, --help       Print help information.
  -V, --version    Print version information.

RUN FLAGS:
  -c, --config <CONFIG>  YAML proxy configuration.
  -h, --help       Print help for the run command.

VALIDATE FLAGS:
  -c, --config <CONFIG>  YAML proxy configuration.
  -h, --help       Print help for the validate command.

CA-INIT FLAGS:
  --cert <CERTIFICATE>  New PEM certificate path. The command will not overwrite it.
  --key <PRIVATE_KEY>   New PEM private-key path. Keep this file secret.
  -h, --help       Print help for the ca-init command.

EXAMPLES:
  modeltap run --config config.yaml
  modeltap validate --config config.yaml
  modeltap ca-init --cert ca-cert.pem --key ca-key.pem"
}

pub fn subcommand_help_text(subcommand: &str) -> Option<&'static str> {
    match subcommand {
        "run" => Some(
            "USAGE:\n  modeltap run --config <CONFIG>\n\nStart the proxy with a YAML configuration file.\n\nFLAGS:\n  -c, --config <CONFIG>  YAML proxy configuration.\n  -h, --help             Print this help text.\n\nEXAMPLE:\n  modeltap run --config config.yaml",
        ),
        "validate" => Some(
            "USAGE:\n  modeltap validate --config <CONFIG>\n\nValidate YAML, site/egress rules, and pricing rules without starting the proxy.\n\nFLAGS:\n  -c, --config <CONFIG>  YAML proxy configuration.\n  -h, --help             Print this help text.\n\nEXAMPLE:\n  modeltap validate --config config.yaml",
        ),
        "ca-init" => Some(
            "USAGE:\n  modeltap ca-init --cert <CERTIFICATE> --key <PRIVATE_KEY>\n\nCreate a new local root CA certificate and private key.\n\nFLAGS:\n  --cert <CERTIFICATE>  New PEM certificate path. The command will not overwrite it.\n  --key <PRIVATE_KEY>   New PEM private-key path. Keep this file secret.\n  -h, --help            Print this help text.\n\nEXAMPLE:\n  modeltap ca-init --cert ca-cert.pem --key ca-key.pem",
        ),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help {
        subcommand: Option<String>,
    },
    Version,
    Run {
        config_file: String,
    },
    Validate {
        config_file: String,
    },
    CaInit {
        certificate_file: String,
        key_file: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliError {
    #[error("usage: modeltap [COMMAND] [ARGUMENTS]; run `modeltap --help` for details")]
    Usage,
    #[error("usage: modeltap run --config <CONFIG>")]
    RunUsage,
    #[error("usage: modeltap validate --config <CONFIG>")]
    ValidateUsage,
    #[error("usage: modeltap ca-init --cert <CERTIFICATE> --key <PRIVATE_KEY>")]
    CaInitUsage,
}

pub fn parse_arguments<I, S>(arguments: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let arguments: Vec<String> = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect();
    match arguments.as_slice() {
        [_, help] if help == "--help" || help == "-h" || help == "help" => {
            Ok(Command::Help { subcommand: None })
        }
        [_, version] if version == "--version" || version == "-V" => Ok(Command::Version),
        [_, command, help]
            if command == "run" && (help == "--help" || help == "-h" || help == "help") =>
        {
            Ok(Command::Help {
                subcommand: Some(command.clone()),
            })
        }
        [_, command, help]
            if command == "validate" && (help == "--help" || help == "-h" || help == "help") =>
        {
            Ok(Command::Help {
                subcommand: Some(command.clone()),
            })
        }
        [_, command, help]
            if command == "ca-init" && (help == "--help" || help == "-h" || help == "help") =>
        {
            Ok(Command::Help {
                subcommand: Some(command.clone()),
            })
        }
        [_, command, options @ ..] if command == "run" => (options.len() == 2)
            .then(|| option_value(options, "-c", "--config"))
            .flatten()
            .map(|config_file| Command::Run { config_file })
            .ok_or(CliError::RunUsage),
        [_, command, options @ ..] if command == "validate" => (options.len() == 2)
            .then(|| option_value(options, "-c", "--config"))
            .flatten()
            .map(|config_file| Command::Validate { config_file })
            .ok_or(CliError::ValidateUsage),
        [_, command, options @ ..] if command == "ca-init" => {
            let certificate_file = (options.len() == 4)
                .then(|| option_value(options, "", "--cert"))
                .flatten();
            let key_file = (options.len() == 4)
                .then(|| option_value(options, "", "--key"))
                .flatten();
            match (certificate_file, key_file) {
                (Some(certificate_file), Some(key_file)) => Ok(Command::CaInit {
                    certificate_file,
                    key_file,
                }),
                _ => Err(CliError::CaInitUsage),
            }
        }
        [_, config_file] => Ok(Command::Run {
            config_file: config_file.clone(),
        }),
        _ => Err(CliError::Usage),
    }
}

fn option_value(options: &[String], short: &str, long: &str) -> Option<String> {
    options
        .windows(2)
        .find(|pair| pair[0] == long || (!short.is_empty() && pair[0] == short))
        .and_then(|pair| (!pair[1].starts_with('-')).then(|| pair[1].clone()))
}
