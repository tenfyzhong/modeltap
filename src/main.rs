use modeltap::cli::{Command, VERSION, help_text, parse_arguments, subcommand_help_text};
use modeltap::config::Config;
use modeltap::mitm::MitmAuthority;
use modeltap::pricing::PriceBook;
use modeltap::telemetry::Telemetry;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let command = match parse_arguments(arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = execute(command).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn execute(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Help { subcommand } => {
            println!(
                "{}",
                subcommand
                    .as_deref()
                    .and_then(subcommand_help_text)
                    .unwrap_or_else(help_text)
            );
            Ok(())
        }
        Command::Version => {
            println!("modeltap {VERSION}");
            Ok(())
        }
        Command::CaInit {
            certificate_file,
            key_file,
        } => {
            let authority = MitmAuthority::generate("modeltap local CA")?;
            write_new(&certificate_file, authority.root_certificate_pem()?)?;
            write_new(&key_file, authority.root_private_key_pem()?)?;
            Ok(())
        }
        Command::Validate { config_file } => validate(&config_file),
        Command::Run { config_file } => run(&config_file).await,
    }
}

fn validate(config_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let input = std::fs::read_to_string(config_file)?;
    let config = Config::from_yaml(&input)?;
    PriceBook::from_config(&config.pricing)?;
    println!("configuration is valid: {config_file}");
    Ok(())
}

async fn run(config_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let input = std::fs::read_to_string(config_file)?;
    let config = Arc::new(Config::from_yaml(&input)?);
    modeltap::logging::init(&config.logging)?;
    let prices = Arc::new(PriceBook::from_config(&config.pricing)?);
    let telemetry = config
        .telemetry
        .otlp
        .as_ref()
        .map(Telemetry::otlp_http)
        .transpose()?
        .map(Arc::new);
    let mitm_authority = config
        .tls
        .as_ref()
        .map(|tls| MitmAuthority::from_pem_files(&tls.ca_cert_file, &tls.ca_key_file).map(Arc::new))
        .transpose()?;
    modeltap::proxy::run(config, mitm_authority, telemetry, prices).await?;
    Ok(())
}

fn write_new(path: &str, contents: String) -> Result<(), std::io::Error> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())
}
