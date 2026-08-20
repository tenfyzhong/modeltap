use flate2::{Compress, Compression, FlushCompress};
use modeltap::usage::{
    Provider, StreamUsageParser, TokenUsage, WebSocketUsageParser,
    permessage_deflate_server_no_context_takeover,
};

#[test]
fn maps_deepseek_to_the_openai_compatible_usage_parser() {
    assert_eq!(
        Provider::from_config("deepseek"),
        Some(Provider::OpenAiChat)
    );
}

#[test]
fn parses_openai_chat_final_usage_across_sse_chunks() {
    let mut parser = StreamUsageParser::new(Provider::OpenAiChat);
    assert!(
        parser
            .push(b"data: {\"model\":\"gpt-5-mini\",\"usage\":null}\n\n")
            .is_none()
    );
    assert!(parser
        .push(b"data: {\"model\":\"gpt-5-mini\",\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":30}}}\n\n")
        .is_none());
    let usage = parser.push(b"data: [DONE]\n\n").unwrap();
    assert_eq!(usage.model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 70,
            output: 20,
            cache_read: 30,
            cache_write: 0
        }
    );
}

#[test]
fn parses_anthropic_cumulative_usage_only_after_message_stop() {
    let mut parser = StreamUsageParser::new(Provider::Anthropic);
    assert!(parser.push(b"event: message_start\ndata: {\"message\":{\"model\":\"claude-sonnet\",\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\n").is_none());
    assert!(parser.push(b"event: message_delta\ndata: {\"usage\":{\"input_tokens\":100,\"output_tokens\":25,\"cache_read_input_tokens\":40}}\n\n").is_none());
    let usage = parser
        .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
        .unwrap();
    assert_eq!(usage.model.as_deref(), Some("claude-sonnet"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 100,
            output: 25,
            cache_read: 40,
            cache_write: 0
        }
    );
}

#[test]
fn parses_gemini_usage_when_the_stream_finishes() {
    let mut parser = StreamUsageParser::new(Provider::Gemini);
    assert!(parser.push(b"data: {\"modelVersion\":\"gemini-2.5-flash\",\"usageMetadata\":{\"promptTokenCount\":120,\"cachedContentTokenCount\":20,\"totalTokenCount\":165}}\n\n").is_none());
    let usage = parser.finish().unwrap();
    assert_eq!(usage.model.as_deref(), Some("gemini-2.5-flash"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 100,
            output: 45,
            cache_read: 20,
            cache_write: 0
        }
    );
}

#[test]
fn parses_openai_embedding_json_response() {
    let usage = Provider::OpenAiEmbedding
        .parse_json(
            br#"{"model":"text-embedding-3-small","usage":{"prompt_tokens":8,"total_tokens":8}}"#,
        )
        .unwrap();
    assert_eq!(usage.model.as_deref(), Some("text-embedding-3-small"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 8,
            output: 0,
            cache_read: 0,
            cache_write: 0
        }
    );
}

#[test]
fn parses_a_fragmented_websocket_response_completed_usage_event() {
    let payload = br#"{"type":"response.completed","response":{"model":"gpt-5.6-terra","usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":30}}}}"#;
    let frame = websocket_text_frame(payload);
    let mut parser = WebSocketUsageParser::new(Provider::OpenAiChat);

    assert!(parser.push(&frame[..5]).is_none());
    let usage = parser.push(&frame[5..]).unwrap();

    assert_eq!(usage.model.as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 70,
            output: 20,
            cache_read: 30,
            cache_write: 0,
        }
    );
}

#[test]
fn decompresses_permessage_deflate_websocket_usage_events() {
    let payload = br#"{"type":"response.completed","response":{"model":"gpt-5.6-terra","usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":30}}}}"#;
    let frame = websocket_compressed_text_frame(payload);
    let mut parser = WebSocketUsageParser::with_permessage_deflate(Provider::OpenAiChat, false);

    let usage = parser.push(&frame).unwrap();

    assert_eq!(usage.model.as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(usage.tokens.input, 70);
    assert_eq!(usage.tokens.output, 20);
    assert_eq!(usage.tokens.cache_read, 30);
}

#[test]
fn retains_server_permessage_deflate_context_between_messages() {
    let first = br#"{"type":"response.completed","response":{"model":"gpt-5.6-terra","usage":{"input_tokens":100,"output_tokens":20}}}"#;
    let second = br#"{"type":"response.completed","response":{"model":"gpt-5.6-terra","usage":{"input_tokens":110,"output_tokens":25}}}"#;
    let mut compressor = Compress::new(Compression::fast(), false);
    let first = websocket_frame(0xc1, &deflate_message(&mut compressor, first));
    let second = websocket_frame(0xc1, &deflate_message(&mut compressor, second));
    let mut parser = WebSocketUsageParser::with_permessage_deflate(Provider::OpenAiChat, false);

    assert_eq!(parser.push(&first).unwrap().tokens.input, 100);
    let usage = parser.push(&second).unwrap();

    assert_eq!(usage.tokens.input, 110);
    assert_eq!(usage.tokens.output, 25);
}

#[test]
fn parses_negotiated_server_permessage_deflate_parameters() {
    assert_eq!(
        permessage_deflate_server_no_context_takeover(
            "x-example; mode=test, permessage-deflate; client_max_window_bits; \
             server_no_context_takeover",
        ),
        Some(true)
    );
    assert_eq!(
        permessage_deflate_server_no_context_takeover("permessage-deflate"),
        Some(false)
    );
    assert_eq!(
        permessage_deflate_server_no_context_takeover("x-example; permessage-deflate=yes"),
        None
    );
}

fn websocket_text_frame(payload: &[u8]) -> Vec<u8> {
    websocket_frame(0x81, payload)
}

fn websocket_frame(first_byte: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= u16::MAX as usize);
    let mut frame = vec![first_byte];
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

fn websocket_compressed_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut compressor = Compress::new(Compression::fast(), false);
    websocket_frame(0xc1, &deflate_message(&mut compressor, payload))
}

fn deflate_message(compressor: &mut Compress, payload: &[u8]) -> Vec<u8> {
    let mut compressed = vec![0_u8; payload.len() * 2 + 64];
    let output_before = compressor.total_out();
    compressor
        .compress(payload, &mut compressed, FlushCompress::Sync)
        .unwrap();
    compressed.truncate((compressor.total_out() - output_before) as usize);
    assert!(compressed.ends_with(&[0x00, 0x00, 0xff, 0xff]));
    compressed.truncate(compressed.len() - 4);
    compressed
}
