use crate::config::{Config, SiteConfig};
use crate::egress::EgressConnector;
use crate::logging::{BODY_PREVIEW_LIMIT, body_preview};
use crate::mitm::MitmAuthority;
use crate::pricing::PriceBook;
use crate::telemetry::Telemetry;
use crate::usage::{
    AutoStreamUsageParser, CursorUsageParser, WebSocketUsageParser, auto_parse_json,
    permessage_deflate_server_no_context_takeover,
};
use bytes::Bytes;
use http::header::{
    CONNECTION, CONTENT_TYPE, HOST, PROXY_AUTHORIZATION, SEC_WEBSOCKET_EXTENSIONS, UPGRADE,
    USER_AGENT,
};
use http::{HeaderMap, HeaderValue, Request, Response, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ServerBuilder;
use opentelemetry::KeyValue;
use std::convert::Infallible;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type ProxyBody = BoxBody<Bytes, BoxError>;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid proxy request: {0}")]
    Request(String),
    #[error("egress configuration error: {0}")]
    Egress(String),
}

pub async fn run(
    config: Arc<Config>,
    mitm_authority: Option<Arc<MitmAuthority>>,
    telemetry: Option<Arc<Telemetry>>,
    prices: Arc<PriceBook>,
) -> Result<(), ProxyError> {
    let listener = tokio::net::TcpListener::bind(&config.proxy.listen).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let config = config.clone();
        let mitm_authority = mitm_authority.clone();
        let telemetry = telemetry.clone();
        let prices = prices.clone();
        tokio::spawn(async move {
            let _ =
                handle_connection_with_telemetry(stream, config, mitm_authority, telemetry, prices)
                    .await;
        });
    }
}

pub async fn handle_connection(client: TcpStream, config: Arc<Config>) -> Result<(), ProxyError> {
    handle_connection_with_mitm(client, config, None).await
}

pub async fn handle_connection_with_mitm(
    client: TcpStream,
    config: Arc<Config>,
    mitm_authority: Option<Arc<MitmAuthority>>,
) -> Result<(), ProxyError> {
    handle_connection_with_telemetry(
        client,
        config,
        mitm_authority,
        None,
        Arc::new(
            PriceBook::from_config(&crate::pricing::PricingConfig {
                timezone: "UTC".to_owned(),
                peak_windows: Vec::new(),
                rules: Vec::new(),
            })
            .expect("UTC pricing configuration is valid"),
        ),
    )
    .await
}

async fn handle_connection_with_telemetry(
    mut client: TcpStream,
    config: Arc<Config>,
    mitm_authority: Option<Arc<MitmAuthority>>,
    telemetry: Option<Arc<Telemetry>>,
    prices: Arc<PriceBook>,
) -> Result<(), ProxyError> {
    let request = read_headers(&mut client, 32 * 1024).await?;
    let (method, target) = parse_request_line(&request)?;
    if method != "CONNECT" {
        client
            .write_all(b"HTTP/1.1 501 Not Implemented\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    let authority = http::uri::Authority::from_str(target)
        .map_err(|_| ProxyError::Request(format!("invalid CONNECT target {target}")))?;
    if authority.port_u16().is_none() {
        return Err(ProxyError::Request(
            "CONNECT target must include a port".to_owned(),
        ));
    }
    let site = config.site_for_host(authority.host());
    let connector = connector_for_site(&config, site)?;
    debug!(
        target = target,
        site = site.map(|site| site.id.as_str()).unwrap_or("unconfigured"),
        mitm = site.is_some(),
        "processing CONNECT request"
    );
    if site.is_some() {
        let mitm_authority = mitm_authority.ok_or_else(|| {
            ProxyError::Egress("a MITM authority is required for a configured site".to_owned())
        })?;
        let host = authority.host().to_owned();
        let upstream_base = format!("https://{target}")
            .parse::<Uri>()
            .map_err(|_| ProxyError::Request(format!("invalid upstream URI {target}")))?;
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        client.flush().await?;
        let tls = TlsAcceptor::from(Arc::new(
            mitm_authority
                .server_config_for(&host)
                .map_err(|error| ProxyError::Egress(error.to_string()))?,
        ))
        .accept(client)
        .await
        .map_err(|error| ProxyError::Request(format!("MITM TLS handshake failed: {error}")))?;
        debug!(target = target, "MITM TLS handshake completed");
        let observer = site.and_then(|site| {
            telemetry.as_ref().map(|telemetry| UsageObserver {
                telemetry: telemetry.clone(),
                prices,
                site: site.id.clone(),
                local_processing_attributes: [KeyValue::new("site", site.id.clone())],
                agent_cli: "unknown".to_owned(),
            })
        });
        return serve_mitm_connection(tls, upstream_base, connector, observer).await;
    }
    let mut upstream = match connector.connect(target).await {
        Ok(stream) => stream,
        Err(error) if site.is_some_and(|site| site.direct_fallback) => EgressConnector::direct()
            .connect(target)
            .await
            .map_err(|direct_error| {
                ProxyError::Egress(format!(
                    "egress failed: {error}; direct fallback failed: {direct_error}"
                ))
            })?,
        Err(error) => {
            debug!(target = target, error = %error, "upstream connection failed");
            client
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await?;
            let _ = error;
            return Ok(());
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    client.flush().await?;
    debug!(
        target = target,
        "starting transparent bidirectional forwarding"
    );
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

pub async fn serve_mitm_connection<S>(
    stream: S,
    upstream_base: Uri,
    egress: EgressConnector,
    observer: Option<UsageObserver>,
) -> Result<(), ProxyError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |request| {
        forward_mitm_request(
            request,
            upstream_base.clone(),
            egress.clone(),
            observer.clone(),
        )
    });
    ServerBuilder::new(TokioExecutor::new())
        .serve_connection_with_upgrades(TokioIo::new(stream), service)
        .await
        .map_err(|error| ProxyError::Request(format!("MITM HTTP connection failed: {error}")))
}

async fn forward_mitm_request(
    mut request: Request<Incoming>,
    upstream_base: Uri,
    egress: EgressConnector,
    observer: Option<UsageObserver>,
) -> Result<Response<ProxyBody>, Infallible> {
    let request_started = Instant::now();
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let uri = match Uri::builder()
        .scheme(upstream_base.scheme_str().unwrap_or("https"))
        .authority(
            upstream_base
                .authority()
                .map(|value| value.as_str())
                .unwrap_or_default(),
        )
        .path_and_query(path_and_query)
        .build()
    {
        Ok(uri) => uri,
        Err(_) => {
            return Ok(error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid target URI",
            ));
        }
    };
    *request.uri_mut() = uri;
    request.headers_mut().remove(PROXY_AUTHORIZATION);
    if let Some(authority) = upstream_base.authority() {
        if let Ok(value) = HeaderValue::from_str(authority.as_str()) {
            request.headers_mut().insert(HOST, value);
        }
    }
    let method = request.method().clone();
    let target = request.uri().clone();
    let is_cursor_connect = is_cursor_connect_content_type(request.headers());
    let agent_cli = agent_cli(request.headers());
    let observer = observer.map(|mut observer| {
        observer.agent_cli = agent_cli;
        observer
    });
    let cursor_usage = is_cursor_connect.then(|| Arc::new(Mutex::new(CursorUsageParser::new())));
    let cursor_request_observer = is_cursor_connect.then_some(observer.clone()).flatten();
    let upgrade_requested = websocket_upgrade_requested(&request);
    let client_upgrade = upgrade_requested.then(|| hyper::upgrade::on(&mut request));
    debug!(method = %method, target = %target, "forwarding MITM HTTP request");
    let request_method = method.clone();
    let request_target = target.clone();
    let cursor_request_parser = cursor_usage.clone();
    let request_processing_observer = observer.clone();
    let request = request.map(move |body| {
        body.map_frame(move |frame| {
            if let Some(data) = frame.data_ref() {
                let started = request_processing_observer.as_ref().map(|_| Instant::now());
                debug!(
                    method = %request_method,
                    target = %request_target,
                    bytes = data.len(),
                    content = %body_preview(data, BODY_PREVIEW_LIMIT),
                    "processing request body chunk"
                );
                if let (Some(parser), Some(observer)) = (
                    cursor_request_parser.as_ref(),
                    cursor_request_observer.as_ref(),
                ) {
                    if let Ok(mut parser) = parser.lock() {
                        if let Some(model) = parser.push_request(data) {
                            if !parser.request_reported() {
                                parser.mark_request_reported();
                                observer.record(&model, &crate::usage::TokenUsage::default());
                            }
                        }
                    }
                }
                if let (Some(observer), Some(started)) =
                    (request_processing_observer.as_ref(), started)
                {
                    observer
                        .record_local_processing_duration(local_processing_microseconds(started));
                }
            }
            frame
        })
        .map_err(box_error)
        .boxed()
    });
    match send_upstream_request(&egress, request, upgrade_requested).await {
        Ok(mut response) => {
            if let Some(observer) = observer.as_ref() {
                observer.telemetry.record_response_duration(
                    &observer.site,
                    request_started.elapsed().as_secs_f64(),
                );
            }
            let is_sse = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream"));
            let is_json = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(is_json_content_type);
            debug!(
                method = %method,
                target = %target,
                status = %response.status(),
                sse = is_sse,
                upstream_first_response_ms = request_started.elapsed().as_secs_f64() * 1_000.0,
                "received upstream response headers"
            );
            if response.status() == http::StatusCode::SWITCHING_PROTOCOLS {
                if let Some(client_upgrade) = client_upgrade {
                    let permessage_deflate = response
                        .headers()
                        .get_all(SEC_WEBSOCKET_EXTENSIONS)
                        .iter()
                        .filter_map(|value| value.to_str().ok())
                        .find_map(permessage_deflate_server_no_context_takeover);
                    let upstream_upgrade = hyper::upgrade::on(&mut response);
                    let websocket_observer = observer.clone();
                    tokio::spawn(async move {
                        match tokio::try_join!(client_upgrade, upstream_upgrade) {
                            Ok((client, upstream)) => {
                                if let Err(error) = tunnel_websocket(
                                    TokioIo::new(client),
                                    TokioIo::new(upstream),
                                    websocket_observer,
                                    permessage_deflate,
                                )
                                .await
                                {
                                    debug!(error = %error, "WebSocket upgrade tunnel failed");
                                }
                            }
                            Err(error) => {
                                debug!(error = %error, "WebSocket upgrade handshake failed");
                            }
                        }
                    });
                    return Ok(response.map(|body| body.map_err(box_error).boxed()));
                }
            }
            let mut parser = observer
                .clone()
                .filter(|_| is_sse)
                .map(|observer| (AutoStreamUsageParser::new(), observer));
            let cursor_observer = is_cursor_connect.then_some(observer.clone()).flatten();
            let direct_observer = (!is_sse && !is_cursor_connect && is_json)
                .then_some(observer.clone())
                .flatten();
            let completion_observer = observer.clone();
            let response_processing_observer = observer.clone();
            let mut direct_parse_attempted = false;
            Ok(response
                .map(move |body| {
                    body.map_frame(move |frame| {
                        if let Some(data) = frame.data_ref() {
                            let started = response_processing_observer
                                .as_ref()
                                .map(|_| Instant::now());
                            debug!(
                                bytes = data.len(),
                                content = %body_preview(data, BODY_PREVIEW_LIMIT),
                                "processing response body chunk"
                            );
                            if let (Some((stream, observer)), Some(data)) =
                                (parser.as_mut(), frame.data_ref())
                            {
                                if let Some((_protocol, usage)) = stream.push(data) {
                                    observer.record(
                                        usage.model.as_deref().unwrap_or("unknown"),
                                        &usage.tokens,
                                    );
                                }
                            } else if let (Some(parser), Some(observer), Some(data)) = (
                                cursor_usage.as_ref(),
                                cursor_observer.as_ref(),
                                frame.data_ref(),
                            ) {
                                if let Ok(mut parser) = parser.lock() {
                                    if let Some(usage) = parser.push_response(data) {
                                        let model = usage.model.as_deref().unwrap_or("unknown");
                                        if parser.request_reported() {
                                            observer.record_tokens(model, &usage.tokens);
                                        } else {
                                            parser.mark_request_reported();
                                            observer.record(model, &usage.tokens);
                                        }
                                    }
                                }
                            } else if let (Some(observer), Some(data)) =
                                (direct_observer.as_ref(), frame.data_ref())
                            {
                                if should_parse_direct_json_usage(direct_parse_attempted, data) {
                                    direct_parse_attempted = true;
                                    if let Some((_protocol, usage)) = auto_parse_json(data) {
                                        observer.record(
                                            usage.model.as_deref().unwrap_or("unknown"),
                                            &usage.tokens,
                                        );
                                    }
                                }
                            }
                            if let (Some(observer), Some(started)) =
                                (response_processing_observer.as_ref(), started)
                            {
                                observer.record_local_processing_duration(
                                    local_processing_microseconds(started),
                                );
                            }
                        }
                        frame
                    })
                    .map_err(box_error)
                    .boxed()
                })
                .map(move |body| {
                    MeasuredBody {
                        inner: body,
                        observer: completion_observer,
                        started: request_started,
                    }
                    .boxed()
                }))
        }
        Err(error) => {
            warn!(method = %method, target = %target, error = %error, "MITM upstream request failed");
            Ok(error_response(
                http::StatusCode::BAD_GATEWAY,
                "upstream connection failed",
            ))
        }
    }
}

struct MeasuredBody {
    inner: ProxyBody,
    observer: Option<UsageObserver>,
    started: Instant,
}

impl hyper::body::Body for MeasuredBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }
}

impl Drop for MeasuredBody {
    fn drop(&mut self) {
        if let Some(observer) = self.observer.as_ref() {
            observer.telemetry.record_processing_duration(
                &observer.site,
                self.started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
    }
}

fn local_processing_microseconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000_000.0
}

fn is_json_content_type(value: &str) -> bool {
    let media_type = value
        .split_once(';')
        .map_or(value, |(media_type, _)| media_type)
        .trim();
    media_type.eq_ignore_ascii_case("application/json")
        || media_type.to_ascii_lowercase().ends_with("+json")
}

fn should_parse_direct_json_usage(attempted: bool, data: &[u8]) -> bool {
    !attempted && !data.is_empty()
}

async fn send_upstream_request(
    egress: &EgressConnector,
    mut request: Request<ProxyBody>,
    force_http1: bool,
) -> Result<Response<Incoming>, BoxError> {
    let uri = request.uri().clone();
    let stream = if force_http1 {
        egress.connect_uri_http1(&uri).await?
    } else {
        egress.connect_uri(&uri).await?
    };
    if !force_http1 && stream.negotiated_h2() {
        *request.version_mut() = http::Version::HTTP_2;
        request.headers_mut().remove(HOST);
        let (mut sender, connection) =
            hyper::client::conn::http2::Builder::new(TokioExecutor::new())
                .handshake(TokioIo::new(stream))
                .await?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                debug!(error = %error, "HTTP/2 upstream connection failed");
            }
        });
        return sender.send_request(request).await.map_err(box_error);
    }

    *request.version_mut() = http::Version::HTTP_11;
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    *request.uri_mut() = Uri::builder().path_and_query(path_and_query).build()?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    let connection = connection.with_upgrades();
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            debug!(error = %error, "HTTP/1.1 upstream connection failed");
        }
    });
    sender.send_request(request).await.map_err(box_error)
}

async fn tunnel_websocket<C, U>(
    client: C,
    upstream: U,
    observer: Option<UsageObserver>,
    permessage_deflate: Option<bool>,
) -> io::Result<()>
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let upstream_usage_observer = observer.clone();
    let upstream_processing_observer = observer.clone();
    let client_to_upstream = async move {
        tokio::io::copy(&mut client_read, &mut upstream_write).await?;
        upstream_write.shutdown().await
    };
    let upstream_to_client = async move {
        let mut parser = upstream_usage_observer.map(|observer| {
            let parser = match permessage_deflate {
                Some(server_no_context_takeover) => {
                    WebSocketUsageParser::with_permessage_deflate(server_no_context_takeover)
                }
                None => WebSocketUsageParser::new(),
            };
            (parser, observer)
        });
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let read = upstream_read.read(&mut buffer).await?;
            if read == 0 {
                return client_write.shutdown().await;
            }
            let data = &buffer[..read];
            let started = upstream_processing_observer
                .as_ref()
                .map(|_| Instant::now());
            debug!(
                bytes = data.len(),
                content = %body_preview(data, BODY_PREVIEW_LIMIT),
                "processing WebSocket server frame bytes"
            );
            if let Some((parser, observer)) = parser.as_mut() {
                if let Some((_protocol, usage)) = parser.push(data) {
                    observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                }
            }
            if let (Some(observer), Some(started)) =
                (upstream_processing_observer.as_ref(), started)
            {
                observer.record_local_processing_duration(local_processing_microseconds(started));
            }
            client_write.write_all(data).await?;
        }
    };
    tokio::try_join!(client_to_upstream, upstream_to_client).map(|_| ())
}

#[derive(Clone)]
pub struct UsageObserver {
    telemetry: Arc<Telemetry>,
    prices: Arc<PriceBook>,
    site: String,
    local_processing_attributes: [KeyValue; 1],
    agent_cli: String,
}

impl UsageObserver {
    fn record_local_processing_duration(&self, microseconds: f64) {
        self.telemetry
            .record_local_processing_duration_with_attributes(
                microseconds,
                &self.local_processing_attributes,
            );
    }
    fn record(&self, model: &str, usage: &crate::usage::TokenUsage) {
        self.telemetry
            .record_usage(&self.site, model, &self.agent_cli, usage, &self.prices);
    }

    fn record_tokens(&self, model: &str, usage: &crate::usage::TokenUsage) {
        self.telemetry
            .record_usage_tokens(&self.site, model, &self.agent_cli, usage, &self.prices);
    }
}

fn agent_cli(headers: &HeaderMap) -> String {
    detect_agent_cli(headers).to_owned()
}

fn detect_agent_cli(headers: &HeaderMap) -> &'static str {
    if is_header_true(headers, "x-oh-my-pi")
        || is_header_true(headers, "x-omp")
        || is_header_true(headers, "x-ghost-mode")
        || header_eq_ignore_case(headers, "x-cursor-client-type", "cli")
    {
        return "oh_my_pi";
    }

    if headers.contains_key("x-claude-code-session-id")
        || header_contains(headers, "anthropic-beta", "claude-code")
    {
        return "claude_code";
    }

    if header_eq_ignore_case(headers, "originator", "codex_exec")
        || header_contains(headers, "originator", "codex")
        || headers.contains_key("x-codex-beta-features")
        || headers.contains_key("x-codex-window-id")
        || headers.contains_key("x-codex-turn-metadata")
    {
        return "codex";
    }

    if headers.contains_key("x-gemini-api-privileged-user-id") {
        return "gemini_cli";
    }

    if header_eq_ignore_case(headers, "originator", "opencode") {
        return "opencode";
    }

    if header_eq_ignore_case(headers, "x-opencode-client", "pi")
        || header_eq_ignore_case(headers, "x-openrouter-title", "pi")
        || header_eq_ignore_case(headers, "x-billing-invoke-origin", "pi")
    {
        return "pi";
    }

    if header_eq_ignore_case(headers, "x-interaction-type", "conversation-user")
        || header_eq_ignore_case(headers, "x-interaction-type", "custom-model")
        || header_eq_ignore_case(headers, "x-initiator", "copilot")
    {
        return "github_copilot";
    }

    if is_cursor_connect_content_type(headers) {
        return "cursor";
    }

    if let Some(user_agent) = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
    {
        let ua = user_agent.to_ascii_lowercase();
        if is_oh_my_pi_user_agent(&ua) {
            return "oh_my_pi";
        }
        return builtin_agent_cli_from_ua(&ua);
    }

    "unknown"
}

fn is_header_true(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

fn header_eq_ignore_case(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case(expected))
}

fn header_contains(headers: &HeaderMap, name: &str, substring: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains(substring))
}

fn is_cursor_connect_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.starts_with("application/connect+proto")
                || v.starts_with("application/grpc-web+proto")
        })
}

fn is_oh_my_pi_user_agent(ua: &str) -> bool {
    ua == "omp"
        || ua.starts_with("omp/")
        || ua.starts_with("omp ")
        || ua.contains(" omp/")
        || ua.contains(" (omp/")
        || ua.starts_with("oh-my-pi")
        || ua.starts_with("oh_my_pi")
        || ua.contains(" oh-my-pi")
        || ua.contains(" oh_my_pi")
        || ua.contains(" (oh-my-pi")
        || ua.contains(" (oh_my_pi")
}

fn builtin_agent_cli_from_ua(ua: &str) -> &'static str {
    if ua.starts_with("claude-code")
        || ua.starts_with("claude-cli")
        || ua.contains(" claude-code")
        || ua.contains(" claude-cli")
        || ua.contains("(claude-code")
        || ua.contains("(claude-cli")
    {
        "claude_code"
    } else if ua.starts_with("codex") || ua.contains(" codex") || ua.contains("(codex") {
        "codex"
    } else if ua.contains("geminicli") || ua.contains("gemini-cli") || ua.contains("gemini_cli") {
        "gemini_cli"
    } else if ua.starts_with("opencode") || ua.contains(" opencode") || ua.contains("(opencode") {
        "opencode"
    } else if ua.starts_with("pi ")
        || ua.starts_with("pi/")
        || ua.starts_with("pi (")
        || ua == "pi"
        || ua.contains(" pi ")
        || ua.contains(" pi/")
        || ua.contains(" pi (")
        || ua.contains(" (pi ")
        || ua.contains(" (pi/")
        || ua.contains(" (pi (")
        || ua.contains("pi-coding-agent")
    {
        "pi"
    } else if ua.starts_with("copilot/")
        || ua.starts_with("copilot ")
        || ua.contains(" copilot/")
        || ua.contains(" copilot ")
        || ua.contains("github-copilot")
        || ua.contains("github_copilot")
    {
        "github_copilot"
    } else if ua.contains("amazonq-for-cli")
        || ua.contains("amazon-q/")
        || ua.contains("amazonq/")
        || ua.contains("amazonq ")
    {
        "amazon_q"
    } else if ua.contains("roocode/") || ua.contains("roo-code/") {
        "roo_code"
    } else if ua.contains("qwencode/") || ua.contains("qwen-code/") || ua.contains("qwen/") {
        "qwen_code"
    } else if ua.contains("factory-cli/") || ua.contains("factory-droid/") || ua.contains("droid/")
    {
        "factory_droid"
    } else if ua.contains("charm-crush/") || ua.contains("crush/") {
        "crush"
    } else if ua.contains("kiro-ide/") || ua.contains("kiro/") {
        "kiro"
    } else if ua.contains("qoder-cli") || ua.contains("qoder/") {
        "qoder"
    } else if ua.contains("antigravity/") || ua.contains("antigravity ") {
        "antigravity"
    } else if ua.starts_with("cursor/") || ua.starts_with("cursor ") || ua.contains(" cursor/") {
        "cursor"
    } else {
        "unknown"
    }
}

fn error_response(status: http::StatusCode, message: &'static str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from_static(message.as_bytes()))
                .map_err(|never: Infallible| match never {})
                .boxed(),
        )
        .expect("static error response is valid")
}

fn box_error<E>(error: E) -> BoxError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

fn websocket_upgrade_requested(request: &Request<Incoming>) -> bool {
    request.headers().contains_key(UPGRADE)
        && request
            .headers()
            .get(CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
}

fn connector_for_site(
    config: &Config,
    site: Option<&SiteConfig>,
) -> Result<EgressConnector, ProxyError> {
    let site_id = site.map(|site| site.id.as_str()).unwrap_or("__unknown__");
    let resolved = config
        .egress_for_site(site_id)
        .map_err(|error| ProxyError::Egress(error.to_string()))?;
    match resolved.proxy {
        Some(proxy) => EgressConnector::from_proxy(proxy)
            .map_err(|error| ProxyError::Egress(error.to_string())),
        None => Ok(EgressConnector::direct()),
    }
}

async fn read_headers(stream: &mut TcpStream, limit: usize) -> Result<String, ProxyError> {
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes)
                .map_err(|_| ProxyError::Request("request headers are not UTF-8".to_owned()));
        }
    }
    Err(ProxyError::Request(
        "request headers exceeded limit".to_owned(),
    ))
}

fn parse_request_line(request: &str) -> Result<(&str, &str), ProxyError> {
    let line = request
        .split("\r\n")
        .next()
        .ok_or_else(|| ProxyError::Request("missing request line".to_owned()))?;
    let mut parts = line.split_ascii_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ProxyError::Request("missing request method".to_owned()))?;
    let target = parts
        .next()
        .ok_or_else(|| ProxyError::Request("missing request target".to_owned()))?;
    let version = parts
        .next()
        .ok_or_else(|| ProxyError::Request("missing HTTP version".to_owned()))?;
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err(ProxyError::Request("invalid request line".to_owned()));
    }
    Ok((method, target))
}
#[cfg(test)]
mod tests {
    use super::{
        agent_cli, is_json_content_type, should_parse_direct_json_usage, tunnel_websocket,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn identifies_agent_from_characteristic_headers() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-oh-my-pi", "true".parse().unwrap());
        assert_eq!(agent_cli(&headers), "oh_my_pi");

        let mut headers = http::HeaderMap::new();
        headers.insert("x-omp", "1".parse().unwrap());
        assert_eq!(agent_cli(&headers), "oh_my_pi");

        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            "application/connect+proto".parse().unwrap(),
        );
        headers.insert("x-ghost-mode", "true".parse().unwrap());
        assert_eq!(agent_cli(&headers), "oh_my_pi");

        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            "application/connect+proto".parse().unwrap(),
        );
        headers.insert("x-cursor-client-type", "cli".parse().unwrap());
        assert_eq!(agent_cli(&headers), "oh_my_pi");

        let mut headers = http::HeaderMap::new();
        headers.insert("x-claude-code-session-id", "test-session".parse().unwrap());
        assert_eq!(agent_cli(&headers), "claude_code");

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            "claude-code-20250219,prompt-caching".parse().unwrap(),
        );
        assert_eq!(agent_cli(&headers), "claude_code");

        let mut headers = http::HeaderMap::new();
        headers.insert("originator", "codex_exec".parse().unwrap());
        assert_eq!(agent_cli(&headers), "codex");

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-beta-features",
            "remote_compaction_v2".parse().unwrap(),
        );
        assert_eq!(agent_cli(&headers), "codex");

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-gemini-api-privileged-user-id",
            "user-123".parse().unwrap(),
        );
        assert_eq!(agent_cli(&headers), "gemini_cli");

        let mut headers = http::HeaderMap::new();
        headers.insert("originator", "opencode".parse().unwrap());
        assert_eq!(agent_cli(&headers), "opencode");

        let mut headers = http::HeaderMap::new();
        headers.insert("x-opencode-client", "pi".parse().unwrap());
        assert_eq!(agent_cli(&headers), "pi");

        let mut headers = http::HeaderMap::new();
        headers.insert("X-OpenRouter-Title", "pi".parse().unwrap());
        assert_eq!(agent_cli(&headers), "pi");

        let mut headers = http::HeaderMap::new();
        headers.insert("X-BILLING-INVOKE-ORIGIN", "Pi".parse().unwrap());
        assert_eq!(agent_cli(&headers), "pi");

        let mut headers = http::HeaderMap::new();
        headers.insert("x-interaction-type", "conversation-user".parse().unwrap());
        assert_eq!(agent_cli(&headers), "github_copilot");

        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            "application/connect+proto".parse().unwrap(),
        );
        assert_eq!(agent_cli(&headers), "cursor");
    }

    #[test]
    fn identifies_builtin_agent_cli_user_agents() {
        for (user_agent, expected) in [
            ("claude-cli/2.1.239 (external, sdk-cli)", "claude_code"),
            ("claude-code/2.1.89 (cli)", "claude_code"),
            ("codex_exec/0.149.0 (Ubuntu 24.4.0; x86_64)", "codex"),
            ("codex_cli_rs/1.0", "codex"),
            (
                "GeminiCLI-tui/0.56.0/simulated-model (linux; x64; GitHub)",
                "gemini_cli",
            ),
            ("GeminiCLI/0.34.0/gemini-pro", "gemini_cli"),
            ("gemini-cli/1.0", "gemini_cli"),
            (
                "opencode/1.18.21 (linux 6.17.0-1022-azure; x64)",
                "opencode",
            ),
            ("OpenCode/1.0", "opencode"),
            ("pi (darwin 24.0; arm64)", "pi"),
            ("pi/0.20.0 (darwin; bun/1.1.20; arm64)", "pi"),
            ("pi-coding-agent", "pi"),
            ("omp/17.3.7", "oh_my_pi"),
            ("omp/0.10.0", "oh_my_pi"),
            ("oh-my-pi/0.2.0", "oh_my_pi"),
            ("copilot/0.0.353 (win32)", "github_copilot"),
            ("AmazonQ-For-CLI/1.0", "amazon_q"),
            ("RooCode/3.53.0", "roo_code"),
            ("QwenCode/0.21.15 (linux; x64)", "qwen_code"),
            ("QwenCode/0.14.0 (linux; x64)", "qwen_code"),
            ("factory-cli/0.62.1", "factory_droid"),
            ("Charm-Crush/0.1", "crush"),
            ("kiro-ide/1.0", "kiro"),
            ("Qoder-Cli/1.0", "qoder"),
            ("antigravity/2.0.1 darwin/arm64", "antigravity"),
        ] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::USER_AGENT, user_agent.parse().unwrap());
            assert_eq!(agent_cli(&headers), expected, "failed for {user_agent}");
        }
    }

    #[test]
    fn does_not_falsely_identify_unrelated_user_agents() {
        for user_agent in [
            "openapi/3.0.0",
            "libomp/18.1.8",
            "stomp/1.2",
            "fastapi (0.110.0)",
            "my-api (python)",
            "curl/8.7.1",
            "python-requests/2.31.0",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        ] {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::USER_AGENT, user_agent.parse().unwrap());
            assert_eq!(
                agent_cli(&headers),
                "unknown",
                "should not match {user_agent}"
            );
        }
    }
    #[test]
    fn identifies_json_media_types_without_matching_other_response_bodies() {
        assert!(is_json_content_type("application/json; charset=utf-8"));
        assert!(is_json_content_type("application/problem+json"));
        assert!(!is_json_content_type("text/plain"));
        assert!(!is_json_content_type("application/octet-stream"));
    }

    #[test]
    fn attempts_direct_json_usage_parsing_once_for_non_empty_body_data() {
        assert!(should_parse_direct_json_usage(false, b"{"));
        assert!(!should_parse_direct_json_usage(true, b"{"));
        assert!(!should_parse_direct_json_usage(false, b""));
    }

    #[tokio::test]
    async fn websocket_tunnel_forwards_server_frames() {
        let (tunnel_client, mut client) = tokio::io::duplex(64 * 1024);
        let (tunnel_upstream, mut upstream) = tokio::io::duplex(64 * 1024);
        let tunnel = tokio::spawn(tunnel_websocket(tunnel_client, tunnel_upstream, None, None));
        let payload = vec![b'x'; 64 * 1024];

        client.shutdown().await.unwrap();
        upstream.write_all(&payload).await.unwrap();
        upstream.shutdown().await.unwrap();

        let mut forwarded = vec![0; payload.len()];
        client.read_exact(&mut forwarded).await.unwrap();
        tunnel.await.unwrap().unwrap();

        assert_eq!(forwarded, payload);
    }
}
