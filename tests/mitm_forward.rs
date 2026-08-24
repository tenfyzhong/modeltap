use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use modeltap::config::Config;
use modeltap::egress::EgressConnector;
use modeltap::mitm::MitmAuthority;
use modeltap::proxy::{handle_connection_with_mitm, serve_mitm_connection};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::io::Write;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[tokio::test]
async fn mitm_forwards_an_http_request_and_streaming_sse_response() {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /v1/chat/completions HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("accept-encoding: identity")
        );
        stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 10\r\n\r\ndata: ok\n\n").await.unwrap();
    });

    let authority = MitmAuthority::generate("modeltap test root").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mitm_address = listener.local_addr().unwrap();
    let server_config = authority.server_config_for("api.openai.com").unwrap();
    let forwarder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let stream = TlsAcceptor::from(Arc::new(server_config))
            .accept(stream)
            .await
            .unwrap();
        serve_mitm_connection(
            stream,
            format!("http://{upstream_address}").parse().unwrap(),
            EgressConnector::direct(),
            None,
        )
        .await
        .unwrap();
    });

    let mut roots = RootCertStore::empty();
    roots.add(authority.root_certificate()).unwrap();
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut client = TlsConnector::from(Arc::new(client_config))
        .connect(
            ServerName::try_from("api.openai.com").unwrap(),
            TcpStream::connect(mitm_address).await.unwrap(),
        )
        .await
        .unwrap();
    client.write_all(b"GET /v1/chat/completions HTTP/1.1\r\nHost: api.openai.com\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.contains("200 OK"));
    assert!(response.ends_with("data: ok\n\n"));
    forwarder.await.unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn mitm_forwards_websocket_upgrades_and_frames() {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /backend-api/codex/responses HTTP/1.1"));
        assert!(request.to_ascii_lowercase().contains("upgrade: websocket"));
        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Extensions: permessage-deflate; server_no_context_takeover\r\n\r\n",
            )
            .await
            .unwrap();
        let mut payload = [0_u8; 4];
        stream.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"ping");
        stream.write_all(b"pong").await.unwrap();
    });

    let authority = MitmAuthority::generate("modeltap test root").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mitm_address = listener.local_addr().unwrap();
    let server_config = authority.server_config_for("chatgpt.com").unwrap();
    let forwarder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let stream = TlsAcceptor::from(Arc::new(server_config))
            .accept(stream)
            .await
            .unwrap();
        serve_mitm_connection(
            stream,
            format!("http://{upstream_address}").parse().unwrap(),
            EgressConnector::direct(),
            None,
        )
        .await
        .unwrap();
    });

    let mut roots = RootCertStore::empty();
    roots.add(authority.root_certificate()).unwrap();
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut client = TlsConnector::from(Arc::new(client_config))
        .connect(
            ServerName::try_from("chatgpt.com").unwrap(),
            TcpStream::connect(mitm_address).await.unwrap(),
        )
        .await
        .unwrap();
    client
        .write_all(
            b"GET /backend-api/codex/responses HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: test\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        client.read_exact(&mut byte).await.unwrap();
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let response = String::from_utf8(response).unwrap();
    assert!(response.contains("101 Switching Protocols"));
    assert!(
        response
            .contains("sec-websocket-extensions: permessage-deflate; server_no_context_takeover")
    );
    client.write_all(b"ping").await.unwrap();
    let mut response = [0_u8; 4];
    client.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"pong");

    forwarder.await.unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn connect_mitm_forwards_through_configured_cascading_proxy() {
    let authority = Arc::new(MitmAuthority::generate("modeltap test root").unwrap());
    let mut ca_file = NamedTempFile::new().unwrap();
    ca_file
        .write_all(authority.root_certificate_pem().unwrap().as_bytes())
        .unwrap();
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    let upstream = tokio::spawn({
        let authority = authority.clone();
        async move {
            let (mut stream, _) = upstream_listener.accept().await.unwrap();
            let mut connect = Vec::new();
            loop {
                let mut byte = [0_u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                connect.push(byte[0]);
                if connect.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            assert!(
                String::from_utf8(connect)
                    .unwrap()
                    .starts_with("CONNECT api.openai.com:443 HTTP/1.1")
            );
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut server_config = authority.server_config_for("api.openai.com").unwrap();
            server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
            let mut stream = TlsAcceptor::from(Arc::new(server_config))
                .accept(stream)
                .await
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0_u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .starts_with("GET /v1/models HTTP/1.1")
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await
                .unwrap();
        }
    });

    let config = Arc::new(
        Config::from_yaml(&format!(
            "egress:\n  default: gost\n  proxies:\n    - id: gost\n      url: http://{upstream_address}\n      target_tls:\n        ca_file: {}\nsites:\n  - id: openai\n    hosts: [api.openai.com]\npricing: {{timezone: Asia/Shanghai}}\n",
            ca_file.path().display()
        ))
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    let proxy = tokio::spawn({
        let authority = authority.clone();
        async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_connection_with_mitm(stream, config, Some(authority))
                .await
                .unwrap();
        }
    });

    let mut roots = RootCertStore::empty();
    roots.add(authority.root_certificate()).unwrap();
    let mut client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec()];
    let mut client = TcpStream::connect(proxy_address).await.unwrap();
    client
        .write_all(b"CONNECT api.openai.com:443 HTTP/1.1\r\nHost: api.openai.com:443\r\n\r\n")
        .await
        .unwrap();
    let mut connect_response = [0_u8; 39];
    client.read_exact(&mut connect_response).await.unwrap();
    assert_eq!(
        &connect_response,
        b"HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    let tls = TlsConnector::from(Arc::new(client_config))
        .connect(ServerName::try_from("api.openai.com").unwrap(), client)
        .await
        .unwrap();
    let (mut sender, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    let response = sender
        .send_request(
            Request::builder()
                .uri("/v1/models")
                .header("host", "api.openai.com")
                .body(Empty::<Bytes>::new())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "ok"
    );
    drop(sender);

    proxy.await.unwrap();
    upstream.await.unwrap();
}
#[tokio::test]
async fn mitm_forwards_chunked_json_and_completes_usage() {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        // Send part 1 of json
        let part1 = br#"{"id":"chatcmpl-1","object":"chat.completion","model":"gpt-4o","choices":[{"message":{"role":"assistant","content":"hello"}}],"#;
        stream
            .write_all(format!("{:X}\r\n", part1.len()).as_bytes())
            .await
            .unwrap();
        stream.write_all(part1).await.unwrap();
        stream.write_all(b"\r\n").await.unwrap();

        // Send part 2 of json with usage
        let part2 = br#""usage":{"prompt_tokens":100,"completion_tokens":25,"total_tokens":125}}"#;
        stream
            .write_all(format!("{:X}\r\n", part2.len()).as_bytes())
            .await
            .unwrap();
        stream.write_all(part2).await.unwrap();
        stream.write_all(b"\r\n").await.unwrap();

        // Send end chunk
        stream.write_all(b"0\r\n\r\n").await.unwrap();
    });

    let authority = MitmAuthority::generate("modeltap test root").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mitm_address = listener.local_addr().unwrap();
    let server_config = authority.server_config_for("api.openai.com").unwrap();
    let forwarder = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let stream = TlsAcceptor::from(Arc::new(server_config))
            .accept(stream)
            .await
            .unwrap();
        serve_mitm_connection(
            stream,
            format!("http://{upstream_address}").parse().unwrap(),
            EgressConnector::direct(),
            None,
        )
        .await
        .unwrap();
    });

    let mut roots = RootCertStore::empty();
    roots.add(authority.root_certificate()).unwrap();
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut client = TlsConnector::from(Arc::new(client_config))
        .connect(
            ServerName::try_from("api.openai.com").unwrap(),
            TcpStream::connect(mitm_address).await.unwrap(),
        )
        .await
        .unwrap();
    client.write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: api.openai.com\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(response.contains("200 OK"));
    assert!(response.contains("\"usage\""));
    forwarder.await.unwrap();
    upstream.await.unwrap();
}
