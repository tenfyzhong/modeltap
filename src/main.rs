use modeltap::cli::{Command, VERSION, help_text, parse_arguments};
use modeltap::config::Config;
use modeltap::mitm::MitmAuthority;
use modeltap::pricing::PriceBook;
use modeltap::telemetry::Telemetry;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().collect();
    match parse_arguments(arguments)? {
        Command::Help => {
            println!("{}", help_text());
            return Ok(());
        }
        Command::Version => {
            println!("modeltap {VERSION}");
            return Ok(());
        }
        Command::CaInit {
            certificate_file,
            key_file,
        } => {
            let authority = MitmAuthority::generate("modeltap local CA")?;
            write_new(&certificate_file, authority.root_certificate_pem()?)?;
            write_new(&key_file, authority.root_private_key_pem()?)?;
            return Ok(());
        }
        Command::Run { config_file } => run(&config_file).await,
    }
}

async fn run(config_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let input = std::fs::read_to_string(config_file)?;
    let config = Arc::new(Config::from_yaml(&input)?);
    modeltap::logging::init(config.logging.level);
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
