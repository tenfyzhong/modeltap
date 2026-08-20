use crate::config::{Config, SiteConfig};
use crate::egress::{EgressConnector, EgressError, UpstreamStream};
use crate::logging::{BODY_PREVIEW_LIMIT, body_preview};
use crate::mitm::MitmAuthority;
use crate::pricing::PriceBook;
use crate::telemetry::Telemetry;
use crate::usage::{
    Provider, StreamUsageParser, WebSocketUsageParser,
    permessage_deflate_server_no_context_takeover,
};
use bytes::Bytes;
use http::header::{CONNECTION, HOST, PROXY_AUTHORIZATION, SEC_WEBSOCKET_EXTENSIONS, UPGRADE};
use http::{HeaderValue, Request, Response, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ServerBuilder;
use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tower_service::Service;
use tracing::debug;

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
        mitm = site.is_some_and(|site| site.mitm),
        "processing CONNECT request"
    );
    if site.is_some_and(|site| site.mitm) {
        let mitm_authority = mitm_authority.ok_or_else(|| {
            ProxyError::Egress("a MITM authority is required for a MITM-enabled site".to_owned())
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
            telemetry.as_ref().and_then(|telemetry| {
                Provider::from_config(&site.provider).map(|provider| UsageObserver {
                    telemetry: telemetry.clone(),
                    prices,
                    site: site.id.clone(),
                    provider_name: site.provider.clone(),
                    provider,
                })
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
    let client: Client<_, ProxyBody> =
        Client::builder(TokioExecutor::new()).build(RoutedConnector {
            egress: egress.clone(),
            http1_only: false,
        });
    let websocket_client: Client<_, ProxyBody> =
        Client::builder(TokioExecutor::new()).build(RoutedConnector {
            egress,
            http1_only: true,
        });
    let service = service_fn(move |request| {
        forward_mitm_request(
            request,
            upstream_base.clone(),
            client.clone(),
            websocket_client.clone(),
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
    client: Client<RoutedConnector, ProxyBody>,
    websocket_client: Client<RoutedConnector, ProxyBody>,
    observer: Option<UsageObserver>,
) -> Result<Response<ProxyBody>, Infallible> {
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
    let upgrade_requested = websocket_upgrade_requested(&request);
    let client_upgrade = upgrade_requested.then(|| hyper::upgrade::on(&mut request));
    debug!(method = %method, target = %target, "forwarding MITM HTTP request");
    let request_method = method.clone();
    let request_target = target.clone();
    let request = request.map(move |body| {
        body.map_frame(move |frame| {
            if let Some(data) = frame.data_ref() {
                debug!(
                    method = %request_method,
                    target = %request_target,
                    bytes = data.len(),
                    content = %body_preview(data, BODY_PREVIEW_LIMIT),
                    "processing request body chunk"
                );
            }
            frame
        })
        .map_err(box_error)
        .boxed()
    });
    let request_started = Instant::now();
    let forwarding_client = if upgrade_requested {
        websocket_client
    } else {
        client
    };
    match forwarding_client.request(request).await {
        Ok(mut response) => {
            if let Some(observer) = observer.as_ref() {
                observer.telemetry.record_response_duration(
                    &observer.site,
                    &observer.provider_name,
                    request_started.elapsed().as_secs_f64(),
                );
            }
            let is_sse = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream"));
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
                }
            }
            let mut parser = observer.clone().filter(|_| is_sse).map(|observer| {
                (
                    observer.provider,
                    StreamUsageParser::new(observer.provider),
                    observer,
                )
            });
            let direct_observer = (!is_sse).then_some(observer).flatten();
            let mut direct_recorded = false;
            Ok(response.map(move |body| {
                body.map_frame(move |frame| {
                    if let Some(data) = frame.data_ref() {
                        debug!(
                            bytes = data.len(),
                            content = %body_preview(data, BODY_PREVIEW_LIMIT),
                            "processing response body chunk"
                        );
                    }
                    if let (Some((_, stream, observer)), Some(data)) =
                        (parser.as_mut(), frame.data_ref())
                    {
                        if let Some(usage) = stream.push(data) {
                            observer
                                .record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                        }
                    } else if let (Some(observer), Some(data)) =
                        (direct_observer.as_ref(), frame.data_ref())
                    {
                        if !direct_recorded {
                            if let Some(usage) = observer.provider.parse_json(data) {
                                observer.record(
                                    usage.model.as_deref().unwrap_or("unknown"),
                                    &usage.tokens,
                                );
                                direct_recorded = true;
                            }
                        }
                    }
                    frame
                })
                .map_err(box_error)
                .boxed()
            }))
        }
        Err(error) => {
            debug!(method = %method, target = %target, error = %error, "MITM upstream request failed");
            Ok(error_response(
                http::StatusCode::BAD_GATEWAY,
                "upstream connection failed",
            ))
        }
    }
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
    let client_to_upstream = async move {
        tokio::io::copy(&mut client_read, &mut upstream_write).await?;
        upstream_write.shutdown().await
    };
    let upstream_to_client = async move {
        let mut parser = observer.clone().map(|observer| {
            let parser = match permessage_deflate {
                Some(server_no_context_takeover) => WebSocketUsageParser::with_permessage_deflate(
                    observer.provider,
                    server_no_context_takeover,
                ),
                None => WebSocketUsageParser::new(observer.provider),
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
            debug!(
                bytes = data.len(),
                content = %body_preview(data, BODY_PREVIEW_LIMIT),
                "processing WebSocket server frame bytes"
            );
            if let Some((parser, observer)) = parser.as_mut() {
                if let Some(usage) = parser.push(data) {
                    observer.record(usage.model.as_deref().unwrap_or("unknown"), &usage.tokens);
                }
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
    provider_name: String,
    provider: Provider,
}

impl UsageObserver {
    fn record(&self, model: &str, usage: &crate::usage::TokenUsage) {
        self.telemetry
            .record_usage(&self.site, &self.provider_name, model, usage, &self.prices);
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

#[derive(Clone)]
struct RoutedConnector {
    egress: EgressConnector,
    http1_only: bool,
}

impl Service<Uri> for RoutedConnector {
    type Response = TokioIo<UpstreamStream>;
    type Error = EgressError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let egress = self.egress.clone();
        let http1_only = self.http1_only;
        Box::pin(async move {
            let stream = if http1_only {
                egress.connect_uri_http1(&uri).await
            } else {
                egress.connect_uri(&uri).await
            }?;
            Ok(TokioIo::new(stream))
        })
    }
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
