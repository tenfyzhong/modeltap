use thiserror::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn help_text() -> &'static str {
    "modeltap - AI API MITM proxy and usage monitor

USAGE:
  modeltap [COMMAND] [ARGUMENTS]

SUBCOMMANDS:
  run <CONFIG>                         Start the proxy with a YAML configuration file.
  ca-init <CERTIFICATE> <PRIVATE_KEY>  Create a new local root CA certificate and private key.
  help                                 Print this help text.

GLOBAL FLAGS:
  -h, --help       Print help information.
  -V, --version    Print version information.

RUN FLAGS:
  -h, --help       Print help for the run command.

CA-INIT FLAGS:
  -h, --help       Print help for the ca-init command.

ARGUMENTS:
  <CONFIG>         YAML proxy configuration. The legacy `modeltap <CONFIG>` form is supported.
  <CERTIFICATE>    New PEM certificate path. The command will not overwrite an existing file.
  <PRIVATE_KEY>    New PEM private key path. Keep this file secret.

ENVIRONMENT:
  Set `logging.level: debug` in the YAML configuration to print processing diagnostics.

EXAMPLES:
  modeltap run config.yaml
  modeltap ca-init ca-cert.pem ca-key.pem"
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Run {
        config_file: String,
    },
    CaInit {
        certificate_file: String,
        key_file: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliError {
    #[error("invalid command or arguments; run `modeltap --help` for usage")]
    Usage,
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
        [_, help] if help == "--help" || help == "-h" || help == "help" => Ok(Command::Help),
        [_, version] if version == "--version" || version == "-V" => Ok(Command::Version),
        [_, command, help]
            if command == "run" && (help == "--help" || help == "-h" || help == "help") =>
        {
            Ok(Command::Help)
        }
        [_, command, help]
            if command == "ca-init" && (help == "--help" || help == "-h" || help == "help") =>
        {
            Ok(Command::Help)
        }
        [_, command, config_file] if command == "run" => Ok(Command::Run {
            config_file: config_file.clone(),
        }),
        [_, command, certificate_file, key_file] if command == "ca-init" => Ok(Command::CaInit {
            certificate_file: certificate_file.clone(),
            key_file: key_file.clone(),
        }),
        [_, config_file] => Ok(Command::Run {
            config_file: config_file.clone(),
        }),
        _ => Err(CliError::Usage),
    }
}
