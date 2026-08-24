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
    hosts: [api.openai.com]
  - id: internal
    hosts: [llm.internal.example]
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
fn parses_an_optional_log_file_path() {
    let config = Config::from_yaml(
        "logging:\n  level: info\n  file: ./logs/modeltap.log\npricing: {timezone: Asia/Shanghai}\n",
    )
    .unwrap();

    assert_eq!(
        config.logging.file,
        Some(std::path::PathBuf::from("./logs/modeltap.log"))
    );
}

#[test]
fn site_protocols_are_detected_without_provider_type_configuration() {
    let config = Config::from_yaml(
        "sites:\n  - id: grok\n    hosts: [api.x.ai]\npricing: {timezone: UTC}\n",
    )
    .unwrap();
    assert_eq!(config.sites[0].id, "grok");

    let configured_protocol = Config::from_yaml(
        "sites:\n  - id: grok\n    provider_type: openai\n    hosts: [api.x.ai]\npricing: {timezone: UTC}\n",
    );
    assert!(configured_protocol.is_err());
}

#[test]
fn site_configurations_always_require_mitm_and_reject_a_mitm_toggle() {
    Config::from_yaml(
        "sites:\n  - id: openai\n    hosts: [api.openai.com]\npricing: {timezone: UTC}\n",
    )
    .unwrap();

    let toggled = Config::from_yaml(
        "sites:\n  - id: openai\n    hosts: [api.openai.com]\n    mitm: false\npricing: {timezone: UTC}\n",
    );
    assert!(toggled.is_err());
}

#[test]
fn config_sample_is_valid_without_provider_type_configuration() {
    let config = Config::from_yaml(include_str!("../config.sample.yaml")).unwrap();
    let grok = config.sites.iter().find(|site| site.id == "grok").unwrap();

    assert_eq!(config.proxy.listen, "127.0.0.1:2080");
    assert_eq!(grok.hosts, ["api.x.ai"]);
}

#[test]
fn defaults_the_proxy_listener_to_port_2080() {
    let config = Config::from_yaml("pricing: {timezone: Asia/Shanghai}\n").unwrap();

    assert_eq!(config.proxy.listen, "127.0.0.1:2080");
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
fn supports_rule_specific_fast_pricing() {
    let config = Config::from_yaml(
        "pricing:\n  timezone: UTC\n  rules:\n    - model: gpt-5-codex\n      currency: USD\n      rates:\n        input: '1.5'\n        output: '6'\n      fast:\n        input: '3'\n        output: '12'\n        cache_read: '0.3'\n",
    )
    .unwrap();
    let prices = PriceBook::from_config(&config.pricing).unwrap();
    let instant = "2026-08-20T10:00:00Z".parse().unwrap();

    let standard = prices.lookup("openai", "gpt-5-codex", instant).unwrap();
    assert_eq!(standard.rate(TokenType::Input).unwrap().to_string(), "1.5");
    assert_eq!(standard.rate(TokenType::Output).unwrap().to_string(), "6");

    let fast = prices
        .lookup_fast("openai", "gpt-5-codex", instant)
        .unwrap();
    assert_eq!(fast.rate(TokenType::Input).unwrap().to_string(), "3");
    assert_eq!(fast.rate(TokenType::Output).unwrap().to_string(), "12");
    assert_eq!(fast.rate(TokenType::CacheRead).unwrap().to_string(), "0.3");
}

#[test]
fn rejects_unknown_egress_and_overlapping_hosts() {
    let unknown = CONFIG.replace("egress: direct\n", "egress: missing\n");
    assert!(Config::from_yaml(&unknown).is_err());

    let overlapping = CONFIG.replace(
        "  - id: internal\n",
        "  - id: duplicate\n    hosts: [api.openai.com]\n  - id: internal\n",
    );
    assert!(Config::from_yaml(&overlapping).is_err());
}

#[test]
fn matches_a_configured_domain_and_all_of_its_subdomains() {
    let config = Config::from_yaml(
        "sites:\n  - id: gemini\n    hosts: [googleapis.com]\npricing: {timezone: UTC}\n",
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
    let config = "sites:\n  - id: gemini\n    hosts: [googleapis.com]\n  - id: other\n    hosts: [generativelanguage.googleapis.com]\npricing: {timezone: UTC}\n";

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
    assert_eq!(price.rate_f64(TokenType::Input), Some(2.5));
    assert_eq!(price.rate_f64(TokenType::Output), Some(10.0));
}

#[test]
fn supports_day_of_week_peak_windows_and_model_level_overrides() {
    let yaml = r#"
pricing:
  timezone: Asia/Shanghai
  peak_windows:
    # 1=Monday, 2=Tuesday, 3=Wednesday, 4=Thursday, 5=Friday
    - weekdays: [1, 2, 3, 4, 5]
      start: "09:00"
      end: "12:00"
    - weekdays: [1, 2, 3, 4, 5]
      start: "14:00"
      end: "18:00"
  rules:
    # Uses global peak windows (Mon-Fri 09:00-12:00, 14:00-18:00)
    - model: "deepseek-default-*"
      currency: USD
      peak:
        input: 1.0
        output: 2.0
      off_peak:
        input: 0.5
        output: 1.0
    # Overrides with custom weekend peak windows (6=Saturday, 7=Sunday)
    - model: "weekend-model-*"
      currency: USD
      peak_windows:
        - weekdays: [6, 7]
          start: "10:00"
          end: "14:00"
      peak:
        input: 3.0
        output: 6.0
      off_peak:
        input: 1.5
        output: 3.0
    # Overrides with Friday/Saturday (5, 6) midnight-crossing window
    - model: "custom-night-*"
      currency: USD
      peak_windows:
        - weekdays: [5, 6]
          start: "22:00"
          end: "02:00"
      peak:
        input: 4.0
        output: 8.0
      off_peak:
        input: 2.0
        output: 4.0
"#;
    let config = Config::from_yaml(yaml).expect("valid config");
    let book = PriceBook::from_config(&config.pricing).expect("valid price book");

    // Thursday 10:00 Shanghai (02:00 UTC) -> Thursday (weekday, day 4)
    let thursday_10am = Utc.with_ymd_and_hms(2026, 8, 20, 2, 0, 0).unwrap();
    // Thursday 13:00 Shanghai (05:00 UTC)
    let thursday_1pm = Utc.with_ymd_and_hms(2026, 8, 20, 5, 0, 0).unwrap();
    // Friday 23:00 Shanghai (15:00 UTC)
    let friday_11pm = Utc.with_ymd_and_hms(2026, 8, 21, 15, 0, 0).unwrap();
    // Saturday 01:00 Shanghai (17:00 UTC Friday)
    let saturday_1am = Utc.with_ymd_and_hms(2026, 8, 21, 17, 0, 0).unwrap();
    // Saturday 11:00 Shanghai (03:00 UTC)
    let saturday_11am = Utc.with_ymd_and_hms(2026, 8, 22, 3, 0, 0).unwrap();
    // Sunday 11:00 Shanghai (03:00 UTC)
    let sunday_11am = Utc.with_ymd_and_hms(2026, 8, 23, 3, 0, 0).unwrap();
    // Sunday 15:00 Shanghai (07:00 UTC)
    let sunday_3pm = Utc.with_ymd_and_hms(2026, 8, 23, 7, 0, 0).unwrap();

    // 1. deepseek-default (inherits global Mon-Fri 09-12, 14-18)
    let d1 = book
        .lookup("any", "deepseek-default-1", thursday_10am)
        .unwrap();
    assert_eq!(d1.period, PricePeriod::Peak);
    assert_eq!(d1.rate(TokenType::Input).unwrap().to_string(), "1");

    let d2 = book
        .lookup("any", "deepseek-default-1", thursday_1pm)
        .unwrap();
    assert_eq!(d2.period, PricePeriod::OffPeak);
    assert_eq!(d2.rate(TokenType::Input).unwrap().to_string(), "0.5");

    let d3 = book
        .lookup("any", "deepseek-default-1", saturday_11am)
        .unwrap();
    assert_eq!(d3.period, PricePeriod::OffPeak);
    assert_eq!(d3.rate(TokenType::Input).unwrap().to_string(), "0.5");

    // 2. weekend-model (overrides with Sat-Sun [6, 7] 10:00-14:00)
    let w1 = book
        .lookup("any", "weekend-model-1", thursday_10am)
        .unwrap();
    assert_eq!(w1.period, PricePeriod::OffPeak);
    assert_eq!(w1.rate(TokenType::Input).unwrap().to_string(), "1.5");

    let w2 = book
        .lookup("any", "weekend-model-1", saturday_11am)
        .unwrap();
    assert_eq!(w2.period, PricePeriod::Peak);
    assert_eq!(w2.rate(TokenType::Input).unwrap().to_string(), "3");

    let w3 = book.lookup("any", "weekend-model-1", sunday_11am).unwrap();
    assert_eq!(w3.period, PricePeriod::Peak);

    let w4 = book.lookup("any", "weekend-model-1", sunday_3pm).unwrap();
    assert_eq!(w4.period, PricePeriod::OffPeak);

    // 3. custom-night (overrides with Fri-Sat [5, 6] 22:00-02:00)
    let n1 = book.lookup("any", "custom-night-1", friday_11pm).unwrap();
    assert_eq!(n1.period, PricePeriod::Peak);

    let n2 = book.lookup("any", "custom-night-1", saturday_1am).unwrap();
    assert_eq!(n2.period, PricePeriod::Peak);

    let n3 = book.lookup("any", "custom-night-1", thursday_10am).unwrap();
    assert_eq!(n3.period, PricePeriod::OffPeak);
}

#[test]
fn detects_peak_window_overlaps_per_day_and_across_midnight() {
    // Same time on different days is valid (no overlap)
    let valid_yaml = r#"
pricing:
  timezone: UTC
  peak_windows:
    - weekdays: [1, 3, 5]
      start: "09:00"
      end: "12:00"
    - weekdays: [2, 4]
      start: "09:00"
      end: "12:00"
  rules:
    - model: "*"
      currency: USD
      rates: { input: 1.0 }
"#;
    assert!(Config::from_yaml(valid_yaml).is_ok());

    // Overlap on same day (Wednesday = 3 is in both)
    let overlap_yaml = r#"
pricing:
  timezone: UTC
  peak_windows:
    - weekdays: [1, 2, 3]
      start: "09:00"
      end: "12:00"
    - weekdays: [3, 4, 5]
      start: "11:00"
      end: "14:00"
  rules:
    - model: "*"
      currency: USD
      rates: { input: 1.0 }
"#;
    assert!(Config::from_yaml(overlap_yaml).is_err());

    // Invalid day number (e.g. 0 or 8)
    let invalid_day_yaml = r#"
pricing:
  timezone: UTC
  peak_windows:
    - weekdays: [0, 8]
      start: "09:00"
      end: "12:00"
  rules:
    - model: "*"
      currency: USD
      rates: { input: 1.0 }
"#;
    assert!(Config::from_yaml(invalid_day_yaml).is_err());
}

#[test]
fn supports_weekdays_aliases_days_of_week_and_day_of_week() {
    let yaml = r#"
pricing:
  timezone: UTC
  peak_windows:
    - days_of_week: [1, 2, 3, 4, 5]
      start: "09:00"
      end: "12:00"
    - day_of_week: [6, 7]
      start: "14:00"
      end: "18:00"
  rules:
    - model: "*"
      currency: USD
      rates: { input: 1.0 }
"#;
    assert!(Config::from_yaml(yaml).is_ok());
}

#[test]
fn supports_weekday_name_strings_short_and_full_and_mixed() {
    let yaml = r#"
pricing:
  timezone: Asia/Shanghai
  rules:
    - model: "model-short-names"
      currency: USD
      peak_windows:
        - weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri"]
          start: "09:00"
          end: "12:00"
      peak: { input: 1.0 }
      off_peak: { input: 0.5 }
    - model: "model-full-names"
      currency: USD
      peak_windows:
        - weekdays: ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"]
          start: "09:00"
          end: "12:00"
      peak: { input: 1.0 }
      off_peak: { input: 0.5 }
    - model: "model-mixed-and-case-insensitive"
      currency: USD
      peak_windows:
        - weekdays: ["mon", 2, "WEDNESDAY", 4, "Fri"]
          start: "09:00"
          end: "12:00"
      peak: { input: 1.0 }
      off_peak: { input: 0.5 }
"#;
    let config = Config::from_yaml(yaml).expect("valid config with weekday names");
    let book = PriceBook::from_config(&config.pricing).expect("valid price book");

    let thursday_10am = Utc.with_ymd_and_hms(2026, 8, 20, 2, 0, 0).unwrap(); // Thursday (day 4) 10:00 Shanghai
    let saturday_10am = Utc.with_ymd_and_hms(2026, 8, 22, 2, 0, 0).unwrap(); // Saturday (day 6) 10:00 Shanghai

    assert_eq!(
        book.lookup("any", "model-short-names", thursday_10am)
            .unwrap()
            .period,
        PricePeriod::Peak
    );
    assert_eq!(
        book.lookup("any", "model-short-names", saturday_10am)
            .unwrap()
            .period,
        PricePeriod::OffPeak
    );

    assert_eq!(
        book.lookup("any", "model-full-names", thursday_10am)
            .unwrap()
            .period,
        PricePeriod::Peak
    );
    assert_eq!(
        book.lookup("any", "model-full-names", saturday_10am)
            .unwrap()
            .period,
        PricePeriod::OffPeak
    );

    assert_eq!(
        book.lookup("any", "model-mixed-and-case-insensitive", thursday_10am)
            .unwrap()
            .period,
        PricePeriod::Peak
    );
    assert_eq!(
        book.lookup("any", "model-mixed-and-case-insensitive", saturday_10am)
            .unwrap()
            .period,
        PricePeriod::OffPeak
    );

    // Invalid weekday string
    let invalid_name_yaml = r#"
pricing:
  timezone: UTC
  peak_windows:
    - weekdays: ["Monday", "Funday"]
      start: "09:00"
      end: "12:00"
  rules:
    - model: "*"
      currency: USD
      rates: { input: 1.0 }
"#;
    assert!(Config::from_yaml(invalid_name_yaml).is_err());
}

#[test]
fn config_sample_uses_official_deepseek_v4_prices_converted_to_usd() {
    let config = Config::from_yaml(include_str!("../config.sample.yaml")).unwrap();
    assert_eq!(config.egress_for_site("deepseek").unwrap().id, "direct");
    assert_eq!(config.site_for_host("api.x.ai").unwrap().id, "grok");
    assert_eq!(config.site_for_host("api2.cursor.sh").unwrap().id, "cursor");
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

    let weekend_during_peak_window = Utc.with_ymd_and_hms(2026, 8, 22, 2, 0, 0).unwrap();
    let flash_weekend = book
        .lookup("deepseek", "deepseek-v4-flash", weekend_during_peak_window)
        .unwrap();
    assert_eq!(flash_weekend.currency, "USD");
    assert_eq!(flash_weekend.period, PricePeriod::OffPeak);
    assert_eq!(
        flash_weekend.rate(TokenType::Input).unwrap().to_string(),
        "0.222610842"
    );
}

#[test]
fn config_sample_prices_cursor_models_at_official_upstream_rates() {
    let config = Config::from_yaml(include_str!("../config.sample.yaml")).unwrap();
    let book = PriceBook::from_config(&config.pricing).unwrap();
    let instant = Utc.with_ymd_and_hms(2026, 8, 20, 2, 0, 0).unwrap();

    let grok = book
        .lookup("cursor", "cursor-grok-4.6-high", instant)
        .unwrap();
    assert_eq!(grok.currency, "USD");
    assert_eq!(grok.rate(TokenType::Input).unwrap().to_string(), "2");
    assert_eq!(grok.rate(TokenType::CacheRead).unwrap().to_string(), "0.5");
    assert_eq!(grok.rate(TokenType::Output).unwrap().to_string(), "6");

    let grok_code = book.lookup("cursor", "grok-code-fast-1", instant).unwrap();
    assert_eq!(grok_code.rate(TokenType::Input).unwrap().to_string(), "1");
    assert_eq!(
        grok_code.rate(TokenType::CacheRead).unwrap().to_string(),
        "0.2"
    );
    assert_eq!(grok_code.rate(TokenType::Output).unwrap().to_string(), "2");

    let gpt = book.lookup("cursor", "gpt-5.2-high", instant).unwrap();
    assert_eq!(gpt.rate(TokenType::Input).unwrap().to_string(), "1.75");
    assert_eq!(gpt.rate(TokenType::CacheRead).unwrap().to_string(), "0.175");
    assert_eq!(gpt.rate(TokenType::Output).unwrap().to_string(), "14");

    let gemini = book
        .lookup("cursor", "gemini-3.7-flash-high", instant)
        .unwrap();
    assert_eq!(gemini.rate(TokenType::Input).unwrap().to_string(), "0.75");
    assert_eq!(
        gemini.rate(TokenType::CacheRead).unwrap().to_string(),
        "0.075"
    );
    assert_eq!(gemini.rate(TokenType::Output).unwrap().to_string(), "3.75");

    // gpt-5.6-sol on cursor uses the cursor-specific override
    let cursor_sol = book.lookup("cursor", "gpt-5.6-sol-high", instant).unwrap();
    assert_eq!(
        cursor_sol.rate(TokenType::Input).unwrap().to_string(),
        "2.5"
    );
    assert_eq!(
        cursor_sol.rate(TokenType::CacheRead).unwrap().to_string(),
        "0.25"
    );
    assert_eq!(
        cursor_sol.rate(TokenType::Output).unwrap().to_string(),
        "15"
    );

    let cursor_sol_fast = book
        .lookup_fast("cursor", "gpt-5.6-sol-high", instant)
        .unwrap();
    assert_eq!(
        cursor_sol_fast.rate(TokenType::Input).unwrap().to_string(),
        "5"
    );
    assert_eq!(
        cursor_sol_fast.rate(TokenType::Output).unwrap().to_string(),
        "30"
    );

    // gpt-5.6-sol on openai uses global official pricing
    let openai_sol = book.lookup("openai", "gpt-5.6-sol-high", instant).unwrap();
    assert_eq!(openai_sol.rate(TokenType::Input).unwrap().to_string(), "5");
    assert_eq!(
        openai_sol.rate(TokenType::CacheRead).unwrap().to_string(),
        "0.5"
    );
    assert_eq!(
        openai_sol.rate(TokenType::Output).unwrap().to_string(),
        "30"
    );

    // claude-sonnet models on cursor and anthropic both use global pricing
    let cursor_claude = book
        .lookup("cursor", "claude-4.6-sonnet-medium", instant)
        .unwrap();
    assert_eq!(
        cursor_claude.rate(TokenType::Input).unwrap().to_string(),
        "3"
    );
    assert_eq!(
        cursor_claude.rate(TokenType::Output).unwrap().to_string(),
        "15"
    );
    assert_eq!(
        cursor_claude
            .rate(TokenType::CacheRead)
            .unwrap()
            .to_string(),
        "0.3"
    );
    assert_eq!(
        cursor_claude
            .rate(TokenType::CacheWrite)
            .unwrap()
            .to_string(),
        "3.75"
    );

    let anthropic_claude = book
        .lookup("anthropic", "claude-sonnet-4-6", instant)
        .unwrap();
    assert_eq!(
        anthropic_claude.rate(TokenType::Input).unwrap().to_string(),
        "3"
    );
    assert_eq!(
        anthropic_claude
            .rate(TokenType::Output)
            .unwrap()
            .to_string(),
        "15"
    );
    assert_eq!(
        anthropic_claude
            .rate(TokenType::CacheRead)
            .unwrap()
            .to_string(),
        "0.3"
    );
    assert_eq!(
        anthropic_claude
            .rate(TokenType::CacheWrite)
            .unwrap()
            .to_string(),
        "3.75"
    );

    // gpt-5.5-{extra-high,high,low,medium,none} brace expansion matches all variants
    for variant in ["extra-high", "high", "low", "medium", "none"] {
        let model = format!("gpt-5.5-{variant}");
        let price = book
            .lookup("cursor", &model, instant)
            .unwrap_or_else(|| panic!("expected {model} to match"));
        assert_eq!(price.rate(TokenType::Input).unwrap().to_string(), "2.5");
        assert_eq!(price.rate(TokenType::Output).unwrap().to_string(), "15");
        assert_eq!(
            price.rate(TokenType::CacheRead).unwrap().to_string(),
            "0.25"
        );
    }
    assert!(book.lookup("cursor", "gpt-5.5-unknown", instant).is_none());
}

#[test]
fn supports_global_pricing_rules_and_site_overrides() {
    let config = Config::from_yaml(
        r#"
pricing:
  timezone: UTC
  rules:
    - model: "claude-4.6-connect"
      currency: USD
      rates:
        input: 3.0
        output: 15.0
    - site: cursor
      model: "claude-4.6-connect"
      currency: USD
      rates:
        input: 2.5
        output: 12.5
    - model: "gpt-5*"
      currency: USD
      rates:
        input: 1.5
        output: 6.0
    - site: custom
      model: "gpt-5-mini"
      currency: USD
      rates:
        input: 0.5
        output: 2.0
"#,
    )
    .unwrap();

    let prices = PriceBook::from_config(&config.pricing).unwrap();
    let instant = Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap();

    // claude-4.6-connect on cursor uses the site override
    let cursor_claude = prices
        .lookup("cursor", "claude-4.6-connect", instant)
        .unwrap();
    assert_eq!(
        cursor_claude.rate(TokenType::Input).unwrap().to_string(),
        "2.5"
    );
    assert_eq!(
        cursor_claude.rate(TokenType::Output).unwrap().to_string(),
        "12.5"
    );

    // claude-4.6-connect on any other site falls back to global pricing
    let anthropic_claude = prices
        .lookup("anthropic", "claude-4.6-connect", instant)
        .unwrap();
    assert_eq!(
        anthropic_claude.rate(TokenType::Input).unwrap().to_string(),
        "3"
    );
    assert_eq!(
        anthropic_claude
            .rate(TokenType::Output)
            .unwrap()
            .to_string(),
        "15"
    );

    let direct_claude = prices
        .lookup("direct", "claude-4.6-connect", instant)
        .unwrap();
    assert_eq!(
        direct_claude.rate(TokenType::Input).unwrap().to_string(),
        "3"
    );
    assert_eq!(
        direct_claude.rate(TokenType::Output).unwrap().to_string(),
        "15"
    );

    // gpt-5-mini on custom uses the site override
    let custom_gpt = prices.lookup("custom", "gpt-5-mini", instant).unwrap();
    assert_eq!(
        custom_gpt.rate(TokenType::Input).unwrap().to_string(),
        "0.5"
    );
    assert_eq!(custom_gpt.rate(TokenType::Output).unwrap().to_string(), "2");

    // gpt-5-mini on openai falls back to global gpt-5* rule
    let openai_gpt = prices.lookup("openai", "gpt-5-mini", instant).unwrap();
    assert_eq!(
        openai_gpt.rate(TokenType::Input).unwrap().to_string(),
        "1.5"
    );
    assert_eq!(openai_gpt.rate(TokenType::Output).unwrap().to_string(), "6");

    // Non-existent model returns None
    assert!(prices.lookup("openai", "unknown-model", instant).is_none());
}

#[test]
fn site_override_precedes_global_regardless_of_rule_order_and_specificity() {
    let config = Config::from_yaml(
        r#"
pricing:
  timezone: UTC
  rules:
    - site: override_site
      model: "claude-*"
      currency: USD
      rates:
        input: 1.0
        output: 2.0
    - model: "claude-4.6-connect"
      currency: USD
      rates:
        input: 3.0
        output: 15.0
    - site: ""
      model: "empty-site-model"
      currency: USD
      rates:
        input: 0.1
        output: 0.2
"#,
    )
    .unwrap();

    let prices = PriceBook::from_config(&config.pricing).unwrap();
    let instant = Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap();

    // override_site with claude-* rule matches before global claude-4.6-connect
    let site_price = prices
        .lookup("override_site", "claude-4.6-connect", instant)
        .unwrap();
    assert_eq!(site_price.rate(TokenType::Input).unwrap().to_string(), "1");
    assert_eq!(site_price.rate(TokenType::Output).unwrap().to_string(), "2");

    // other site gets the exact global rule
    let other_price = prices
        .lookup("other_site", "claude-4.6-connect", instant)
        .unwrap();
    assert_eq!(other_price.rate(TokenType::Input).unwrap().to_string(), "3");
    assert_eq!(
        other_price.rate(TokenType::Output).unwrap().to_string(),
        "15"
    );

    // site: "" is treated as global
    let empty_site_price = prices
        .lookup("any_site", "empty-site-model", instant)
        .unwrap();
    assert_eq!(
        empty_site_price.rate(TokenType::Input).unwrap().to_string(),
        "0.1"
    );
    assert_eq!(
        empty_site_price
            .rate(TokenType::Output)
            .unwrap()
            .to_string(),
        "0.2"
    );
}
