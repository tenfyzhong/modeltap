use modeltap::config::OtlpConfig;
use modeltap::pricing::{PriceBook, PricingConfig};
use modeltap::telemetry::Telemetry;
use modeltap::usage::TokenUsage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exports_usage_metrics_to_the_alloy_otlp_http_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let size = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..size]);
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(&request).starts_with("POST /v1/metrics HTTP/1.1"));
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
        "openai",
        "gpt-test",
        &TokenUsage {
            input: 12,
            ..TokenUsage::default()
        },
        &prices,
    );
    telemetry.record_response_duration("openai", "openai", 0.042);
    telemetry.force_flush().unwrap();
    receiver.await.unwrap();
}
