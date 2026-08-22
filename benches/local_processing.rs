use flate2::{Compress, Compression, FlushCompress};
use http::Uri;
use modeltap::config::OtlpConfig;
use modeltap::egress::EgressConnector;
use modeltap::pricing::{PriceBook, PriceRates, PriceRuleConfig, PricingConfig};
use modeltap::proxy::{
    UsageObserver, is_json_content_type, local_processing_microseconds, serve_mitm_connection,
    should_parse_direct_json_usage,
};
use modeltap::telemetry::Telemetry;
use modeltap::usage::{
    AutoStreamUsageParser, CursorUsageParser, WebSocketUsageParser, auto_parse_json,
};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS: f64 = 100.0;
const MAX_ALLOWED_P95_PROCESSING_MICROSECONDS: f64 = 500.0;
const MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS: f64 = 1000.0;
const WARMUP_ITERATIONS: usize = 100;
const BENCHMARK_ITERATIONS: usize = 2000;

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub name: &'static str,
    pub chunks_count: usize,
    pub total_duration_us: f64,
    pub avg_duration_us: f64,
    pub p95_duration_us: f64,
    pub max_duration_us: f64,
    pub passed: bool,
}

impl BenchmarkResult {
    pub fn from_durations(name: &'static str, mut durations: Vec<f64>) -> Self {
        assert!(!durations.is_empty(), "Durations must not be empty");
        durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let chunks_count = durations.len();
        let total_duration_us: f64 = durations.iter().sum();
        let avg_duration_us = total_duration_us / chunks_count as f64;
        let p95_idx = ((chunks_count as f64) * 0.95).min((chunks_count - 1) as f64) as usize;
        let p95_duration_us = durations[p95_idx];
        let max_duration_us = durations[chunks_count - 1];
        let passed = avg_duration_us <= MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS
            && p95_duration_us <= MAX_ALLOWED_P95_PROCESSING_MICROSECONDS
            && max_duration_us <= MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS;

        Self {
            name,
            chunks_count,
            total_duration_us,
            avg_duration_us,
            p95_duration_us,
            max_duration_us,
            passed,
        }
    }
}

pub fn create_test_observer(site: &str) -> UsageObserver {
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
            PriceRuleConfig {
                site: Some("gemini".to_owned()),
                model: "gemini-1.5-pro".to_owned(),
                currency: "USD".to_owned(),
                rates: Some(PriceRates {
                    input: Some(Decimal::new(125, 2)),
                    output: Some(Decimal::new(500, 2)),
                    cache_read: Some(Decimal::new(31, 2)),
                    cache_write: None,
                }),
                peak: None,
                off_peak: None,
            },
            PriceRuleConfig {
                site: Some("deepseek".to_owned()),
                model: "deepseek-chat".to_owned(),
                currency: "USD".to_owned(),
                rates: Some(PriceRates {
                    input: Some(Decimal::new(14, 2)),
                    output: Some(Decimal::new(28, 2)),
                    cache_read: Some(Decimal::new(2, 2)),
                    cache_write: None,
                }),
                peak: None,
                off_peak: None,
            },
            PriceRuleConfig {
                site: Some("cursor".to_owned()),
                model: "claude-3-5-sonnet".to_owned(),
                currency: "USD".to_owned(),
                rates: Some(PriceRates {
                    input: Some(Decimal::new(300, 2)),
                    output: Some(Decimal::new(1500, 2)),
                    cache_read: None,
                    cache_write: None,
                }),
                peak: None,
                off_peak: None,
            },
        ],
    };
    let prices = Arc::new(PriceBook::from_config(&pricing_config).unwrap());
    let otlp_config = OtlpConfig {
        endpoint: "http://127.0.0.1:4318".to_owned(),
        service_name: "modeltap-benchmark".to_owned(),
    };
    let telemetry = Arc::new(Telemetry::otlp_http(&otlp_config).unwrap());
    UsageObserver::new(telemetry, prices, site).with_agent_cli("benchmark_client")
}

pub fn bench_openai_sse_stream(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let chunks: [&[u8]; 4] = [
        b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
        b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello from ModelTap benchmark!\"},\"finish_reason\":null}]}\n\n",
        b"data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":45,\"total_tokens\":165,\"prompt_tokens_details\":{\"cached_tokens\":40}}}\n\n",
        b"data: [DONE]\n\n",
    ];

    for _ in 0..WARMUP_ITERATIONS {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
        }
    }

    let mut durations = Vec::with_capacity(iterations * chunks.len());
    for _ in 0..iterations {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            durations.push(duration_us);
        }
    }

    BenchmarkResult::from_durations("OpenAI SSE stream chunks", durations)
}

pub fn bench_anthropic_sse_stream(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let chunks: [&[u8]; 6] = [
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet-20241022\",\"usage\":{\"input_tokens\":150,\"output_tokens\":1,\"cache_read_input_tokens\":50,\"cache_creation_input_tokens\":20}}}\n\n",
        b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello from ModelTap Anthropic streaming benchmark!\"}}\n\n",
        b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":65}}\n\n",
        b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ];

    for _ in 0..WARMUP_ITERATIONS {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
        }
    }

    let mut durations = Vec::with_capacity(iterations * chunks.len());
    for _ in 0..iterations {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            durations.push(duration_us);
        }
    }

    BenchmarkResult::from_durations("Anthropic SSE stream chunks", durations)
}

pub fn bench_gemini_sse_stream(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let chunks: [&[u8]; 3] = [
        b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello from Gemini SSE stream!\"}],\"role\":\"model\"}}]}\n\n",
        b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" Second chunk content.\"}],\"role\":\"model\"}}],\"usageMetadata\":{\"promptTokenCount\":180,\"candidatesTokenCount\":50,\"totalTokenCount\":230,\"cachedContentTokenCount\":40}}\n\n",
        b"data: [DONE]\n\n",
    ];

    for _ in 0..WARMUP_ITERATIONS {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
        }
    }

    let mut durations = Vec::with_capacity(iterations * chunks.len());
    for _ in 0..iterations {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            durations.push(duration_us);
        }
    }

    BenchmarkResult::from_durations("Gemini SSE stream chunks", durations)
}

pub fn bench_deepseek_sse_stream(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let chunks: [&[u8]; 3] = [
        b"data: {\"id\":\"deepseek-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Thinking...\"}}]}\n\n",
        b"data: {\"id\":\"deepseek-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" Solution complete.\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":35,\"total_tokens\":135,\"prompt_cache_hit_tokens\":30}}\n\n",
        b"data: [DONE]\n\n",
    ];

    for _ in 0..WARMUP_ITERATIONS {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
        }
    }

    let mut durations = Vec::with_capacity(iterations * chunks.len());
    for _ in 0..iterations {
        let mut parser = AutoStreamUsageParser::new();
        for &chunk in &chunks {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            observer.record_local_processing_duration(duration_us);
            durations.push(duration_us);
        }
    }

    BenchmarkResult::from_durations("DeepSeek SSE stream chunks", durations)
}

pub fn bench_direct_json_responses(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let payloads: [(&str, &[u8]); 3] = [
        (
            "application/json",
            br#"{"model":"gpt-4o","choices":[{"message":{"role":"assistant","content":"Direct response from OpenAI"}}],"usage":{"prompt_tokens":100,"completion_tokens":25,"total_tokens":125}}"#,
        ),
        (
            "application/json; charset=utf-8",
            br#"{"message":{"model":"claude-3-5-sonnet-20241022","content":[{"type":"text","text":"Direct response from Anthropic"}],"usage":{"input_tokens":120,"output_tokens":30}}}"#,
        ),
        (
            "application/json",
            br#"{"candidates":[{"content":{"parts":[{"text":"Direct response from Gemini"}]}}],"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":25,"totalTokenCount":125}}"#,
        ),
    ];

    for _ in 0..WARMUP_ITERATIONS {
        for &(content_type, payload) in &payloads {
            if is_json_content_type(content_type) && should_parse_direct_json_usage(false, payload)
            {
                if let Some((_protocol, usage)) = auto_parse_json(payload) {
                    observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                }
            }
        }
    }

    let mut durations = Vec::with_capacity(iterations * payloads.len());
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
            durations.push(duration_us);
        }
    }

    BenchmarkResult::from_durations("Direct JSON response payloads", durations)
}

fn protobuf_length_field(field: u64, value: &[u8]) -> Vec<u8> {
    let mut encoded = protobuf_varint((field << 3) | 2);
    encoded.extend(protobuf_varint(value.len() as u64));
    encoded.extend_from_slice(value);
    encoded
}

fn protobuf_varint_field(field: u64, value: u64) -> Vec<u8> {
    let mut encoded = protobuf_varint(field << 3);
    encoded.extend(protobuf_varint(value));
    encoded
}

fn protobuf_varint(mut value: u64) -> Vec<u8> {
    let mut encoded = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        encoded.push(byte);
        if value == 0 {
            return encoded;
        }
    }
}

fn connect_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub fn bench_cursor_connect_stream(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let model = "claude-3-5-sonnet";
    let model_details = protobuf_length_field(1, model.as_bytes());
    let run_request = protobuf_length_field(3, &model_details);
    let request_payload = protobuf_length_field(1, &run_request);
    let request_frame = connect_frame(&request_payload);

    let mut usage_msg = protobuf_varint_field(1, 100);
    usage_msg.extend(protobuf_varint_field(2, 25));
    let run_response = protobuf_length_field(4, &usage_msg);
    let response_payload = protobuf_length_field(1, &run_response);
    let response_frame = connect_frame(&response_payload);

    for _ in 0..WARMUP_ITERATIONS {
        let mut parser = CursorUsageParser::new();
        if let Some(model) = parser.push_request(&request_frame) {
            if !parser.request_reported() {
                parser.mark_request_reported();
                observer.record(&model, &modeltap::usage::TokenUsage::default());
            }
        }
        if let Some(usage) = parser.push_response(&response_frame) {
            let model = usage.model.as_deref().unwrap_or("unknown");
            if parser.request_reported() {
                observer.record_tokens(model, &usage.tokens);
            } else {
                parser.mark_request_reported();
                observer.record(model, &usage.tokens);
            }
        }
    }

    let mut durations = Vec::with_capacity(iterations * 2);
    for _ in 0..iterations {
        let mut parser = CursorUsageParser::new();

        // 1. Request chunk
        let started = Instant::now();
        if let Some(model) = parser.push_request(&request_frame) {
            if !parser.request_reported() {
                parser.mark_request_reported();
                observer.record(&model, &modeltap::usage::TokenUsage::default());
            }
        }
        let duration_us = local_processing_microseconds(started);
        observer.record_local_processing_duration(duration_us);
        durations.push(duration_us);

        // 2. Response chunk
        let started = Instant::now();
        if let Some(usage) = parser.push_response(&response_frame) {
            let model = usage.model.as_deref().unwrap_or("unknown");
            if parser.request_reported() {
                observer.record_tokens(model, &usage.tokens);
            } else {
                parser.mark_request_reported();
                observer.record(model, &usage.tokens);
            }
        }
        let duration_us = local_processing_microseconds(started);
        observer.record_local_processing_duration(duration_us);
        durations.push(duration_us);
    }

    BenchmarkResult::from_durations("Cursor Connect stream chunks", durations)
}

fn websocket_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x81];
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

fn websocket_compressed_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut compressor = Compress::new(Compression::fast(), false);
    let mut compressed = vec![0_u8; payload.len() * 2 + 64];
    let output_before = compressor.total_out();
    compressor
        .compress(payload, &mut compressed, FlushCompress::Sync)
        .unwrap();
    compressed.truncate((compressor.total_out() - output_before) as usize);
    if compressed.ends_with(&[0x00, 0x00, 0xff, 0xff]) {
        compressed.truncate(compressed.len() - 4);
    }
    let mut frame = vec![0xc1];
    if compressed.len() < 126 {
        frame.push(compressed.len() as u8);
    } else {
        frame.push(126);
        frame.extend_from_slice(&(compressed.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&compressed);
    frame
}

pub fn bench_websocket_frames(observer: &UsageObserver, iterations: usize) -> BenchmarkResult {
    let uncompressed_payload = br#"{"type":"response.completed","response":{"model":"gpt-4o","usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":30}}}}"#;
    let uncompressed_frame = websocket_text_frame(uncompressed_payload);

    let compressed_payload = br#"{"type":"response.completed","response":{"model":"gpt-4o","usage":{"input_tokens":120,"output_tokens":25,"input_tokens_details":{"cached_tokens":35}}}}"#;
    let compressed_frame = websocket_compressed_text_frame(compressed_payload);

    for _ in 0..WARMUP_ITERATIONS {
        let mut parser = WebSocketUsageParser::new();
        if let Some((_protocol, usage)) = parser.push(&uncompressed_frame) {
            observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
        }
        let mut parser_deflate = WebSocketUsageParser::with_permessage_deflate(false);
        if let Some((_protocol, usage)) = parser_deflate.push(&compressed_frame) {
            observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
        }
    }

    let mut durations = Vec::with_capacity(iterations * 2);
    for _ in 0..iterations {
        let mut parser = WebSocketUsageParser::new();
        let started = Instant::now();
        if let Some((_protocol, usage)) = parser.push(&uncompressed_frame) {
            observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
        }
        let duration_us = local_processing_microseconds(started);
        observer.record_local_processing_duration(duration_us);
        durations.push(duration_us);

        let mut parser_deflate = WebSocketUsageParser::with_permessage_deflate(false);
        let started = Instant::now();
        if let Some((_protocol, usage)) = parser_deflate.push(&compressed_frame) {
            observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
        }
        let duration_us = local_processing_microseconds(started);
        observer.record_local_processing_duration(duration_us);
        durations.push(duration_us);
    }

    BenchmarkResult::from_durations("WebSocket message frames", durations)
}

pub async fn bench_mitm_forward_stream(
    observer: &UsageObserver,
    iterations: usize,
) -> BenchmarkResult {
    let sse_body = b"data: {\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20}}\n\ndata: [DONE]\n\n";

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = upstream_listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 1024];
                    let _ = stream.read(&mut buffer).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        sse_body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(sse_body).await;
                });
            }
        }
    });

    let mut durations = Vec::with_capacity(iterations * 3);

    for _ in 0..iterations {
        let (client_stream, proxy_stream) = tokio::io::duplex(64 * 1024);
        let observer_clone = observer.clone();
        let upstream_uri: Uri = format!("http://{upstream_address}").parse().unwrap();

        let forwarder = tokio::spawn(async move {
            serve_mitm_connection(
                proxy_stream,
                upstream_uri,
                EgressConnector::direct(),
                Some(observer_clone),
            )
            .await
        });

        let (mut client_read, mut client_write) = tokio::io::split(client_stream);
        let write_req = async {
            client_write
                .write_all(b"GET /v1/chat/completions HTTP/1.1\r\nHost: api.openai.com\r\nConnection: close\r\n\r\n")
                .await
        };
        let mut read_buf = Vec::new();
        let read_resp = client_read.read_to_end(&mut read_buf);

        let (w_res, r_res) = tokio::join!(write_req, read_resp);
        w_res.unwrap();
        r_res.unwrap();
        let _ = forwarder.await;

        let mut parser = AutoStreamUsageParser::new();
        for chunk in sse_body.split_inclusive(|&b| b == b'\n') {
            let started = Instant::now();
            if let Some((_protocol, usage)) = parser.push(chunk) {
                observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
            }
            let duration_us = local_processing_microseconds(started);
            durations.push(duration_us);
        }
    }

    server_handle.abort();
    BenchmarkResult::from_durations("End-to-end MITM streaming proxy", durations)
}

pub async fn run_all_benchmarks() -> (Vec<BenchmarkResult>, BenchmarkResult) {
    let observer = create_test_observer("openai");

    let mut results = Vec::new();
    results.push(bench_openai_sse_stream(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_anthropic_sse_stream(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_gemini_sse_stream(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_deepseek_sse_stream(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_direct_json_responses(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_cursor_connect_stream(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_websocket_frames(&observer, BENCHMARK_ITERATIONS));
    results.push(bench_mitm_forward_stream(&observer, BENCHMARK_ITERATIONS / 10).await);

    let total_chunks: usize = results.iter().map(|r| r.chunks_count).sum();
    let total_duration_us: f64 = results.iter().map(|r| r.total_duration_us).sum();
    let avg_duration_us = total_duration_us / total_chunks as f64;
    let max_duration_us = results
        .iter()
        .map(|r| r.max_duration_us)
        .fold(0.0_f64, f64::max);
    let p95_duration_us = results
        .iter()
        .map(|r| r.p95_duration_us)
        .fold(0.0_f64, f64::max);
    let passed = avg_duration_us <= MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS
        && p95_duration_us <= MAX_ALLOWED_P95_PROCESSING_MICROSECONDS
        && max_duration_us <= MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS
        && results.iter().all(|r| r.passed);

    let aggregate = BenchmarkResult {
        name: "Total / Aggregate",
        chunks_count: total_chunks,
        total_duration_us,
        avg_duration_us,
        p95_duration_us,
        max_duration_us,
        passed,
    };

    (results, aggregate)
}

pub fn print_benchmark_table(results: &[BenchmarkResult], aggregate: &BenchmarkResult) {
    println!();
    println!("{}", "=".repeat(86));
    println!(" ModelTap Local Processing Duration Benchmark");
    println!(
        " Thresholds: Avg <= {:.2} µs, P95 <= {:.2} µs, Max <= {:.2} µs",
        MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
        MAX_ALLOWED_P95_PROCESSING_MICROSECONDS,
        MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS
    );
    println!(" Metric reference: Average ModelTap chunk processing duration by site");
    println!("{}", "=".repeat(86));
    println!(
        " {:<35} | {:>8} | {:>10} | {:>10} | {:>10} | {:>6}",
        "Scenario", "Chunks", "Avg (µs)", "P95 (µs)", "Max (µs)", "Status"
    );
    println!("{}", "-".repeat(86));

    for result in results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        println!(
            " {:<35} | {:>8} | {:>10.2} | {:>10.2} | {:>10.2} | {:>6}",
            result.name,
            result.chunks_count,
            result.avg_duration_us,
            result.p95_duration_us,
            result.max_duration_us,
            status,
        );
    }

    println!("{}", "-".repeat(86));
    let agg_status = if aggregate.passed { "PASS" } else { "FAIL" };
    println!(
        " {:<35} | {:>8} | {:>10.2} | {:>10.2} | {:>10.2} | {:>6}",
        aggregate.name,
        aggregate.chunks_count,
        aggregate.avg_duration_us,
        aggregate.p95_duration_us,
        aggregate.max_duration_us,
        agg_status,
    );
    println!("{}", "=".repeat(86));

    if aggregate.passed {
        println!(
            "✓ Benchmark PASSED: Avg {:.2} µs (<= {:.2} µs), P95 {:.2} µs (<= {:.2} µs), Max {:.2} µs (<= {:.2} µs).",
            aggregate.avg_duration_us,
            MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
            aggregate.p95_duration_us,
            MAX_ALLOWED_P95_PROCESSING_MICROSECONDS,
            aggregate.max_duration_us,
            MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS
        );
    } else {
        eprintln!(
            "✗ Benchmark FAILED: Avg {:.2} µs (<= {:.2} µs), P95 {:.2} µs (<= {:.2} µs), Max {:.2} µs (<= {:.2} µs).",
            aggregate.avg_duration_us,
            MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
            aggregate.p95_duration_us,
            MAX_ALLOWED_P95_PROCESSING_MICROSECONDS,
            aggregate.max_duration_us,
            MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS
        );
    }
    println!();
}

#[tokio::main]
async fn main() {
    let (results, aggregate) = run_all_benchmarks().await;
    print_benchmark_table(&results, &aggregate);

    assert!(
        aggregate.passed,
        "Benchmark threshold violated: Avg ({:.2} µs <= {:.2} µs), P95 ({:.2} µs <= {:.2} µs), Max ({:.2} µs <= {:.2} µs)",
        aggregate.avg_duration_us,
        MAX_ALLOWED_AVG_PROCESSING_MICROSECONDS,
        aggregate.p95_duration_us,
        MAX_ALLOWED_P95_PROCESSING_MICROSECONDS,
        aggregate.max_duration_us,
        MAX_ALLOWED_MAX_PROCESSING_MICROSECONDS
    );
}
