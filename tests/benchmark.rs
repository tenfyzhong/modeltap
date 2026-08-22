use modeltap::config::OtlpConfig;
use modeltap::pricing::{PriceBook, PriceRates, PriceRuleConfig, PricingConfig};
use modeltap::proxy::{
    UsageObserver, is_json_content_type, local_processing_microseconds,
    should_parse_direct_json_usage,
};
use modeltap::telemetry::Telemetry;
use modeltap::usage::{AutoStreamUsageParser, WebSocketUsageParser, auto_parse_json};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Instant;

const MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS: f64 = 100.0;

fn create_test_observer(site: &str) -> UsageObserver {
    let pricing_config = PricingConfig {
        timezone: "UTC".to_owned(),
        peak_windows: Vec::new(),
        rules: vec![
            PriceRuleConfig {
                site: Some("openai".to_owned()),
                model: "gpt-4o".to_owned(),
                currency: "USD".to_owned(),
                rates: Some(PriceRates {
                    input: Some(Decimal::new(250, 2)),
                    output: Some(Decimal::new(1000, 2)),
                    cache_read: Some(Decimal::new(125, 2)),
                    cache_write: None,
                }),
                peak: None,
                off_peak: None,
            },
            PriceRuleConfig {
                site: Some("anthropic".to_owned()),
                model: "claude-3-5-sonnet-20241022".to_owned(),
                currency: "USD".to_owned(),
                rates: Some(PriceRates {
                    input: Some(Decimal::new(300, 2)),
                    output: Some(Decimal::new(1500, 2)),
                    cache_read: Some(Decimal::new(30, 2)),
                    cache_write: Some(Decimal::new(375, 2)),
                }),
                peak: None,
                off_peak: None,
            },
        ],
    };
    let prices = Arc::new(PriceBook::from_config(&pricing_config).unwrap());
    let otlp_config = OtlpConfig {
        endpoint: "http://127.0.0.1:4318".to_owned(),
        service_name: "modeltap-test".to_owned(),
    };
    let telemetry = Arc::new(Telemetry::otlp_http(&otlp_config).unwrap());
    UsageObserver::new(telemetry, prices, site).with_agent_cli("test_client")
}

#[test]
fn openai_sse_chunk_processing_duration_is_under_100_microseconds() {
    let observer = create_test_observer("openai");
    let chunks: [&[u8]; 4] = [
        b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
        b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello from ModelTap benchmark!\"},\"finish_reason\":null}]}\n\n",
        b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":45,\"total_tokens\":165}}\n\n",
        b"data: [DONE]\n\n",
    ];

    let iterations = 1000;
    let mut total_duration_us = 0.0;
    let mut total_chunks = 0;

    for _ in 0..iterations {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            total_duration_us += duration_us;
            total_chunks += 1;
        }
    }

    let avg_duration_us = total_duration_us / total_chunks as f64;
    assert!(
        avg_duration_us < MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
        "Average ModelTap local processing duration ({avg_duration_us:.2} µs) exceeded 100 µs limit"
    );
}

#[test]
fn anthropic_sse_chunk_processing_duration_is_under_100_microseconds() {
    let observer = create_test_observer("anthropic");
    let chunks: [&[u8]; 6] = [
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet-20241022\",\"usage\":{\"input_tokens\":150,\"output_tokens\":1}}}\n\n",
        b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello world\"}}\n\n",
        b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":65}}\n\n",
        b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ];

    let iterations = 1000;
    let mut total_duration_us = 0.0;
    let mut total_chunks = 0;

    for _ in 0..iterations {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            total_duration_us += duration_us;
            total_chunks += 1;
        }
    }

    let avg_duration_us = total_duration_us / total_chunks as f64;
    assert!(
        avg_duration_us < MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
        "Average Anthropic chunk processing duration ({avg_duration_us:.2} µs) exceeded 100 µs limit"
    );
}

#[test]
fn direct_json_response_processing_duration_is_under_100_microseconds() {
    let observer = create_test_observer("openai");
    let payloads: [(&str, &[u8]); 2] = [
        (
            "application/json",
            br#"{"model":"gpt-4o","choices":[{"message":{"role":"assistant","content":"Direct response"}}],"usage":{"prompt_tokens":100,"completion_tokens":25,"total_tokens":125}}"#,
        ),
        (
            "application/json; charset=utf-8",
            br#"{"message":{"model":"claude-3-5-sonnet-20241022","content":[{"type":"text","text":"Direct response"}],"usage":{"input_tokens":120,"output_tokens":30}}}"#,
        ),
    ];

    let iterations = 1000;
    let mut total_duration_us = 0.0;
    let mut total_chunks = 0;

    for _ in 0..iterations {
        for &(content_type, payload) in &payloads {
            let started = Instant::now();
            let mut direct_parse_attempted = false;
            if is_json_content_type(content_type)
                && should_parse_direct_json_usage(direct_parse_attempted, payload)
            {
                direct_parse_attempted = true;
                if let Some((_protocol, usage)) = auto_parse_json(payload) {
                    observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                }
            }
            let _ = direct_parse_attempted;
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            total_duration_us += duration_us;
            total_chunks += 1;
        }
    }

    let avg_duration_us = total_duration_us / total_chunks as f64;
    assert!(
        avg_duration_us < MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
        "Average direct JSON processing duration ({avg_duration_us:.2} µs) exceeded 100 µs limit"
    );
}

#[test]
fn websocket_frame_processing_duration_is_under_100_microseconds() {
    let observer = create_test_observer("openai");
    let payload = br#"{"type":"response.completed","response":{"model":"gpt-4o","usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":30}}}}"#;

    let mut frame = vec![0x81, payload.len() as u8];
    frame.extend_from_slice(payload);

    let iterations = 1000;
    let mut total_duration_us = 0.0;
    let mut total_chunks = 0;

    for _ in 0..iterations {
        let mut parser = WebSocketUsageParser::new();
        let started = Instant::now();
        if let Some((_protocol, usage)) = parser.push(&frame) {
            observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
        }
        let duration_us = local_processing_microseconds(started);
        observer.record_local_processing_duration(duration_us);
        total_duration_us += duration_us;
        total_chunks += 1;
    }

    let avg_duration_us = total_duration_us / total_chunks as f64;
    assert!(
        avg_duration_us < MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
        "Average WebSocket frame processing duration ({avg_duration_us:.2} µs) exceeded 100 µs limit"
    );
}
