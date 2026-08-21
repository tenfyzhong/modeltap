use modeltap::config::OtlpConfig;
use modeltap::pricing::{PriceBook, PricingConfig};
use modeltap::telemetry::Telemetry;
use modeltap::usage::TokenUsage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let size = stream.read(&mut buffer).await.unwrap();
        request.extend_from_slice(&buffer[..size]);
        if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
        .unwrap()
        .parse::<usize>()
        .unwrap();
    while request.len() < header_end + content_length {
        let size = stream.read(&mut buffer).await.unwrap();
        request.extend_from_slice(&buffer[..size]);
    }
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exports_usage_metrics_to_the_alloy_otlp_http_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        assert!(String::from_utf8_lossy(&request).starts_with("POST /v1/metrics HTTP/1.1"));
        assert!(
            request
                .windows(b"ai_proxy_processing_duration_microseconds".len())
                .any(|bytes| bytes == b"ai_proxy_processing_duration_microseconds")
        );
        assert!(
            request
                .windows(b"ai_proxy_local_processing_duration_microseconds".len())
                .any(|bytes| bytes == b"ai_proxy_local_processing_duration_microseconds")
        );
        assert!(
            request
                .windows(b"agent_cli".len())
                .any(|bytes| bytes == b"agent_cli")
        );
        assert!(
            request
                .windows(b"codex".len())
                .any(|bytes| bytes == b"codex")
        );
        assert!(
            !request
                .windows(b"provider".len())
                .any(|bytes| bytes == b"provider")
        );
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    let telemetry = Telemetry::otlp_http(&OtlpConfig {
        endpoint: format!("http://{address}"),
        service_name: "modeltap-test".to_owned(),
    })
    .unwrap();
    let prices = PriceBook::from_config(&PricingConfig {
        timezone: "UTC".to_owned(),
        peak_windows: Vec::new(),
        rules: Vec::new(),
    })
    .unwrap();
    telemetry.record_usage(
        "openai",
        "gpt-test",
        "codex",
        &TokenUsage {
            input: 12,
            ..TokenUsage::default()
        },
        &prices,
    );
    telemetry.record_response_duration("openai", 0.042);
    telemetry.record_processing_duration("openai", 123);
    telemetry.record_local_processing_duration("openai", 45);
    telemetry.force_flush().unwrap();
    receiver.await.unwrap();
}
