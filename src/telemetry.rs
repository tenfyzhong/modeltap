use crate::config::OtlpConfig;
use crate::logging::usage_report_summary;
use crate::pricing::{PriceBook, PricePeriod, TokenType};
use crate::usage::TokenUsage;
use chrono::Utc;
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("OTLP exporter setup failed: {0}")]
    Exporter(String),
}

pub struct Telemetry {
    _provider: SdkMeterProvider,
    requests: Counter<u64>,
    tokens: Counter<u64>,
    cost: Counter<f64>,
    upstream_first_response: Histogram<f64>,
    processing_duration: Histogram<f64>,
    local_processing_duration: Histogram<f64>,
    telemetry_record_duration: Histogram<f64>,
}

impl Telemetry {
    pub fn otlp_http(config: &OtlpConfig) -> Result<Self, TelemetryError> {
        let endpoint = format!("{}/v1/metrics", config.endpoint.trim_end_matches('/'));
        let exporter = MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(endpoint)
            .build()
            .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
        let provider = SdkMeterProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name(config.service_name.clone())
                    .build(),
            )
            .with_periodic_exporter(exporter)
            .build();
        let meter = provider.meter("modeltap");
        Ok(Self {
            _provider: provider,
            requests: meter.u64_counter("ai_proxy_requests").build(),
            tokens: meter.u64_counter("ai_proxy_tokens").build(),
            cost: meter.f64_counter("ai_proxy_cost").build(),
            upstream_first_response: meter
                .f64_histogram("ai_proxy_upstream_first_response_seconds")
                .with_description(
                    "Time from receiving a proxied request to upstream response headers",
                )
                .with_unit("s")
                .build(),
            processing_duration: meter
                .f64_histogram("ai_proxy_processing_duration_microseconds")
                .with_description(
                    "Time from a proxied request entering ModelTap until its response leaves ModelTap",
                )
                .with_unit("us")
                .build(),
            local_processing_duration: meter
                .f64_histogram("ai_proxy_local_processing_duration_microseconds")
                .with_description(
                    "Local time spent parsing proxied HTTP bodies and recording usage telemetry",
                )
                .with_unit("us")
                .build(),
            telemetry_record_duration: meter
                .f64_histogram("ai_proxy_telemetry_record_duration_seconds")
                .with_description("Local time spent recording usage telemetry")
                .with_unit("s")
                .build(),
        })
    }

    pub fn record_usage(
        &self,
        site: &str,
        model: &str,
        agent_cli: &str,
        usage: &TokenUsage,
        prices: &PriceBook,
    ) {
        self.record_usage_inner(site, model, agent_cli, usage, prices, true);
    }

    pub fn record_usage_tokens(
        &self,
        site: &str,
        model: &str,
        agent_cli: &str,
        usage: &TokenUsage,
        prices: &PriceBook,
    ) {
        self.record_usage_inner(site, model, agent_cli, usage, prices, false);
    }

    fn record_usage_inner(
        &self,
        site: &str,
        model: &str,
        agent_cli: &str,
        usage: &TokenUsage,
        prices: &PriceBook,
        count_request: bool,
    ) {
        let started = std::time::Instant::now();
        let price = prices.lookup(site, model, Utc::now());
        let period = price.as_ref().map(|price| match price.period {
            PricePeriod::Peak => "peak",
            PricePeriod::OffPeak => "off_peak",
        });
        let currency = price
            .as_ref()
            .map(|price| price.currency.to_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        let base = [
            KeyValue::new("site", site.to_owned()),
            KeyValue::new("model", model.to_owned()),
            KeyValue::new("agent_cli", agent_cli.to_owned()),
        ];
        if count_request {
            self.requests.add(1, &base);
        }
        let mut total_cost = 0.0;
        for (kind, amount, token_type) in [
            ("input", usage.input, TokenType::Input),
            ("output", usage.output, TokenType::Output),
            ("cache_read", usage.cache_read, TokenType::CacheRead),
            ("cache_write", usage.cache_write, TokenType::CacheWrite),
        ] {
            if amount == 0 {
                continue;
            }
            let attributes = [
                base[0].clone(),
                base[1].clone(),
                base[2].clone(),
                KeyValue::new("type", kind),
            ];
            self.tokens.add(amount, &attributes);
            if let Some(price) = price.as_ref().and_then(|price| price.rate_f64(token_type)) {
                let attributes = [
                    base[0].clone(),
                    base[1].clone(),
                    base[2].clone(),
                    KeyValue::new("price_period", period.unwrap_or("unknown")),
                    KeyValue::new("currency", currency.clone()),
                ];
                let value = price * amount as f64 / 1_000_000.0;
                self.cost.add(value, &attributes);
                total_cost += value;
            }
        }
        debug!(
            report = %usage_report_summary(site, model, agent_cli, usage),
            currency = %currency,
            price_period = period.unwrap_or("unknown"),
            cost = total_cost,
            "reporting AI usage telemetry"
        );
        self.telemetry_record_duration.record(
            started.elapsed().as_secs_f64(),
            &[KeyValue::new("signal", "usage")],
        );
    }

    pub fn record_response_duration(&self, site: &str, seconds: f64) {
        self.upstream_first_response
            .record(seconds, &[KeyValue::new("site", site.to_owned())]);
    }

    pub fn record_processing_duration(&self, site: &str, microseconds: u64) {
        self.processing_duration.record(
            microseconds as f64,
            &[KeyValue::new("site", site.to_owned())],
        );
    }

    pub fn record_local_processing_duration(&self, site: &str, agent_cli: &str, microseconds: f64) {
        self.local_processing_duration.record(
            microseconds,
            &[
                KeyValue::new("site", site.to_owned()),
                KeyValue::new("agent_cli", agent_cli.to_owned()),
            ],
        );
    }

    pub fn record_local_processing_duration_with_attributes(
        &self,
        microseconds: f64,
        attributes: &[KeyValue],
    ) {
        self.local_processing_duration
            .record(microseconds, attributes);
    }

    pub fn force_flush(&self) -> Result<(), TelemetryError> {
        self._provider
            .force_flush()
            .map_err(|error| TelemetryError::Exporter(error.to_string()))
    }
}
