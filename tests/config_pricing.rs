use chrono::{TimeZone, Utc};
use modeltap::config::{Config, EgressProtocol, LogLevel};
use modeltap::pricing::{PriceBook, PricePeriod, TokenType};

const CONFIG: &str = r#"
proxy:
  listen: 127.0.0.1:8080
egress:
  default: gost
  proxies:
    - id: gost
      url: http://gost.internal:8080
    - id: secure
      url: https://proxy.internal:8443
      tls:
        server_name: proxy.internal
    - id: socks
      url: socks://socks.internal:1080
sites:
  - id: openai
    provider: openai
    hosts: [api.openai.com]
    mitm: true
  - id: internal
    provider: openai
    hosts: [llm.internal.example]
    mitm: true
    egress: direct
pricing:
  timezone: Asia/Shanghai
  peak_windows: ["09:00-18:00", "22:00-02:00"]
  rules:
    - site: openai
      model: gpt-*
      currency: USD
      peak:
        input: "2.50"
        output: "10"
      off_peak:
        input: "1.25"
        output: "5"
"#;

#[test]
fn parses_standard_egress_protocols_and_site_overrides() {
    let config = Config::from_yaml(CONFIG).expect("valid config");

    assert_eq!(config.egress_for_site("openai").unwrap().id, "gost");
    assert_eq!(config.egress_for_site("internal").unwrap().id, "direct");
    assert_eq!(config.egress_for_site("unknown").unwrap().id, "gost");
    assert_eq!(
        config.egress.proxies[0].protocol().unwrap(),
        EgressProtocol::Http
    );
    assert_eq!(
        config.egress.proxies[1].protocol().unwrap(),
        EgressProtocol::Https
    );
    assert_eq!(
        config.egress.proxies[2].protocol().unwrap(),
        EgressProtocol::Socks5
    );
}

#[test]
fn parses_otlp_http_telemetry_configuration() {
    let config = Config::from_yaml(
        "telemetry:\n  otlp:\n    endpoint: http://alloy:4318\n    service_name: ai-proxy\npricing: {timezone: Asia/Shanghai}\n",
    )
    .unwrap();
    let otlp = config.telemetry.otlp.unwrap();
    assert_eq!(otlp.endpoint, "http://alloy:4318");
    assert_eq!(otlp.service_name, "ai-proxy");
}

#[test]
fn parses_a_configured_debug_log_level() {
    let config =
        Config::from_yaml("logging:\n  level: debug\npricing: {timezone: Asia/Shanghai}\n")
            .unwrap();

    assert_eq!(config.logging.level, LogLevel::Debug);
}

#[test]
fn allows_an_unauthenticated_non_loopback_listener() {
    let config =
        Config::from_yaml("proxy:\n  listen: 0.0.0.0:8080\npricing: {timezone: Asia/Shanghai}\n")
            .unwrap();

    assert_eq!(config.proxy.listen, "0.0.0.0:8080");
}

#[test]
fn supports_explicit_peak_window_bounds_and_uniform_prices() {
    let config = Config::from_yaml(
        "pricing:\n  timezone: UTC\n  peak_windows:\n    - start: '09:00'\n      end: '12:00'\n    - start: '14:00'\n      end: '18:00'\n  rules:\n    - site: openai\n      model: gpt-*\n      currency: USD\n      rates:\n        input: '1.5'\n        output: '6'\n",
    )
    .unwrap();
    let prices = PriceBook::from_config(&config.pricing).unwrap();
    let instant = "2026-08-20T10:00:00Z".parse().unwrap();
    let price = prices.lookup("openai", "gpt-test", instant).unwrap();
    assert_eq!(
        price
            .rate(modeltap::pricing::TokenType::Input)
            .unwrap()
            .to_string(),
        "1.5"
    );
}

#[test]
fn rejects_unknown_egress_and_overlapping_hosts() {
    let unknown = CONFIG.replace("egress: direct\n", "egress: missing\n");
    assert!(Config::from_yaml(&unknown).is_err());

    let overlapping = CONFIG.replace(
        "  - id: internal\n",
        "  - id: duplicate\n    provider: openai\n    hosts: [api.openai.com]\n    mitm: true\n  - id: internal\n",
    );
    assert!(Config::from_yaml(&overlapping).is_err());
}

#[test]
fn matches_a_configured_domain_and_all_of_its_subdomains() {
    let config = Config::from_yaml(
        "sites:\n  - id: gemini\n    provider: gemini\n    hosts: [googleapis.com]\n    mitm: true\npricing: {timezone: UTC}\n",
    )
    .unwrap();

    assert_eq!(config.site_for_host("googleapis.com").unwrap().id, "gemini");
    assert_eq!(
        config
            .site_for_host("generativelanguage.googleapis.com")
            .unwrap()
            .id,
        "gemini"
    );
    assert_eq!(
        config.site_for_host("a.b.googleapis.com.").unwrap().id,
        "gemini"
    );
    assert!(config.site_for_host("notgoogleapis.com").is_none());
    assert!(config.site_for_host("googleapis.com.example").is_none());
}

#[test]
fn rejects_domain_trees_configured_by_multiple_sites() {
    let config = "sites:\n  - id: gemini\n    provider: gemini\n    hosts: [googleapis.com]\n  - id: other\n    provider: gemini\n    hosts: [generativelanguage.googleapis.com]\npricing: {timezone: UTC}\n";

    assert!(Config::from_yaml(config).is_err());
}

#[test]
fn uses_daily_peak_windows_in_the_configured_timezone() {
    let config = Config::from_yaml(CONFIG).expect("valid config");
    let book = PriceBook::from_config(&config.pricing).expect("valid price book");

    let peak = Utc.with_ymd_and_hms(2026, 8, 20, 2, 0, 0).unwrap(); // 10:00 Shanghai
    let off_peak = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap(); // 20:00 Shanghai
    let overnight_peak = Utc.with_ymd_and_hms(2026, 8, 20, 16, 0, 0).unwrap(); // midnight Shanghai

    assert_eq!(book.period_at(peak), PricePeriod::Peak);
    assert_eq!(book.period_at(off_peak), PricePeriod::OffPeak);
    assert_eq!(book.period_at(overnight_peak), PricePeriod::Peak);

    let price = book.lookup("openai", "gpt-5-mini", peak).unwrap();
    assert_eq!(price.rate(TokenType::Input).unwrap().to_string(), "2.50");
    assert_eq!(price.rate(TokenType::Output).unwrap().to_string(), "10");
}

#[test]
fn test_config_uses_official_deepseek_v4_prices_converted_to_usd() {
    let config = Config::from_yaml(include_str!("../config.test.yaml")).unwrap();
    assert_eq!(config.egress_for_site("deepseek").unwrap().id, "direct");
    let book = PriceBook::from_config(&config.pricing).unwrap();
    let peak = Utc.with_ymd_and_hms(2026, 8, 20, 2, 0, 0).unwrap();
    let off_peak = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let flash = book.lookup("deepseek", "deepseek-v4-flash", peak).unwrap();
    assert_eq!(flash.currency, "USD");
    assert_eq!(flash.period, PricePeriod::Peak);
    assert_eq!(
        flash.rate(TokenType::Input).unwrap().to_string(),
        "0.445221684"
    );
    assert_eq!(
        flash.rate(TokenType::CacheRead).unwrap().to_string(),
        "0.014840723"
    );
    assert_eq!(
        flash.rate(TokenType::Output).unwrap().to_string(),
        "1.335665051"
    );

    let pro = book
        .lookup("deepseek", "deepseek-v4-pro", off_peak)
        .unwrap();
    assert_eq!(pro.currency, "USD");
    assert_eq!(pro.period, PricePeriod::OffPeak);
    assert_eq!(
        pro.rate(TokenType::Input).unwrap().to_string(),
        "0.667832526"
    );
    assert_eq!(
        pro.rate(TokenType::CacheRead).unwrap().to_string(),
        "0.022261084"
    );
    assert_eq!(
        pro.rate(TokenType::Output).unwrap().to_string(),
        "2.003497577"
    );
}
