use serde::Deserialize;
use std::collections::HashSet;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid YAML configuration: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid configuration: {0}")]
    Validation(String),
    #[error("invalid proxy URL for {id}: {source}")]
    Url { id: String, source: url::ParseError },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub tls: Option<MitmTlsConfig>,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub egress: EgressConfig,
    #[serde(default)]
    pub sites: Vec<SiteConfig>,
    pub pricing: crate::pricing::PricingConfig,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub level: LogLevel,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MitmTlsConfig {
    pub ca_cert_file: String,
    pub ca_key_file: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub otlp: Option<OtlpConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OtlpConfig {
    pub endpoint: String,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_service_name() -> String {
    "modeltap".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
}

fn default_listen() -> String {
    "127.0.0.1:8080".to_owned()
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EgressConfig {
    #[serde(default = "default_egress")]
    pub default: String,
    #[serde(default)]
    pub proxies: Vec<EgressProxyConfig>,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self {
            default: default_egress(),
            proxies: Vec::new(),
        }
    }
}

fn default_egress() -> String {
    "direct".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
pub struct EgressProxyConfig {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub auth: Option<ProxyAuthConfig>,
    #[serde(default)]
    pub tls: Option<UpstreamTlsConfig>,
    #[serde(default)]
    pub target_tls: Option<UpstreamTlsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyAuthConfig {
    pub username_env: String,
    pub password_env: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamTlsConfig {
    pub server_name: Option<String>,
    pub ca_file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressProtocol {
    Http,
    Https,
    Socks5,
}

impl EgressProxyConfig {
    pub fn parsed_url(&self) -> Result<Url, ConfigError> {
        Url::parse(&self.url).map_err(|source| ConfigError::Url {
            id: self.id.clone(),
            source,
        })
    }

    pub fn protocol(&self) -> Result<EgressProtocol, ConfigError> {
        let url = self.parsed_url()?;
        match url.scheme() {
            "http" => Ok(EgressProtocol::Http),
            "https" => Ok(EgressProtocol::Https),
            "socks" | "socks5" => Ok(EgressProtocol::Socks5),
            scheme => Err(ConfigError::Validation(format!(
                "egress proxy {} uses unsupported scheme {scheme}",
                self.id
            ))),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiteConfig {
    pub id: String,
    pub provider: String,
    pub hosts: Vec<String>,
    #[serde(default)]
    pub mitm: bool,
    pub egress: Option<String>,
    #[serde(default)]
    pub direct_fallback: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedEgress<'a> {
    pub id: &'a str,
    pub proxy: Option<&'a EgressProxyConfig>,
}

impl Config {
    pub fn from_yaml(input: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_yaml::from_str(input)?;
        config.validate()?;
        Ok(config)
    }

    pub fn egress_for_site(&self, site: &str) -> Result<ResolvedEgress<'_>, ConfigError> {
        let egress = self
            .sites
            .iter()
            .find(|configured| configured.id == site)
            .and_then(|configured| configured.egress.as_deref())
            .unwrap_or(&self.egress.default);
        self.resolve_egress(egress)
    }

    pub fn site_for_host(&self, host: &str) -> Option<&SiteConfig> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.sites.iter().find(|site| {
            site.hosts.iter().any(|pattern| {
                let domain = normalized_host_domain(pattern);
                is_same_or_subdomain(&host, &domain)
            })
        })
    }

    pub fn resolve_egress(&self, id: &str) -> Result<ResolvedEgress<'_>, ConfigError> {
        if id == "direct" {
            return Ok(ResolvedEgress {
                id: "direct",
                proxy: None,
            });
        }
        self.egress
            .proxies
            .iter()
            .find(|proxy| proxy.id == id)
            .map(|proxy| ResolvedEgress {
                id: &proxy.id,
                proxy: Some(proxy),
            })
            .ok_or_else(|| ConfigError::Validation(format!("unknown egress proxy {id}")))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut proxy_ids = HashSet::new();
        for proxy in &self.egress.proxies {
            if proxy.id.is_empty() || proxy.id == "direct" || !proxy_ids.insert(&proxy.id) {
                return Err(ConfigError::Validation(format!(
                    "invalid or duplicate egress proxy id {}",
                    proxy.id
                )));
            }
            let url = proxy.parsed_url()?;
            if url.host_str().is_none() || url.port_or_known_default().is_none() {
                return Err(ConfigError::Validation(format!(
                    "egress proxy {} must have a host and port",
                    proxy.id
                )));
            }
            let protocol = proxy.protocol()?;
            if matches!(protocol, EgressProtocol::Https) && proxy.tls.is_none() {
                return Err(ConfigError::Validation(format!(
                    "HTTPS egress proxy {} requires tls configuration",
                    proxy.id
                )));
            }
        }
        self.resolve_egress(&self.egress.default)?;

        let mut site_ids = HashSet::new();
        let mut hosts: Vec<String> = Vec::new();
        for site in &self.sites {
            if site.id.is_empty() || !site_ids.insert(&site.id) {
                return Err(ConfigError::Validation(format!(
                    "invalid or duplicate site id {}",
                    site.id
                )));
            }
            if site.hosts.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "site {} has no hosts",
                    site.id
                )));
            }
            if let Some(egress) = &site.egress {
                self.resolve_egress(egress)?;
            }
            for host in &site.hosts {
                let domain = normalized_host_domain(host);
                if domain.is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "site {} has an empty host domain",
                        site.id
                    )));
                }
                if hosts.iter().any(|configured| {
                    is_same_or_subdomain(&domain, configured)
                        || is_same_or_subdomain(configured, &domain)
                }) {
                    return Err(ConfigError::Validation(format!(
                        "host domain {domain} overlaps another configured host domain"
                    )));
                }
                hosts.push(domain);
            }
        }
        crate::pricing::PriceBook::from_config(&self.pricing)
            .map_err(|error| ConfigError::Validation(error.to_string()))?;
        Ok(())
    }
}

fn normalized_host_domain(value: &str) -> String {
    value
        .trim_end_matches('.')
        .strip_prefix("*.")
        .unwrap_or(value.trim_end_matches('.'))
        .to_ascii_lowercase()
}

fn is_same_or_subdomain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}
