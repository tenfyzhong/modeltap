use flate2::{Compress, Compression, FlushCompress};
use modeltap::usage::{
    AutoStreamUsageParser, CursorUsageParser, Provider, StreamUsageParser, TokenUsage,
    WebSocketUsageParser, auto_parse_json, permessage_deflate_server_no_context_takeover,
};

#[test]
fn automatically_detects_openai_and_anthropic_usage_payloads() {
    let (protocol, openai) = auto_parse_json(
        br#"{"model":"deepseek-chat","usage":{"prompt_tokens":100,"completion_tokens":25}}"#,
    )
    .unwrap();
    assert_eq!(protocol, Provider::OpenAiChat);
    assert_eq!(openai.tokens.input, 100);
    assert_eq!(openai.tokens.output, 25);

    let (protocol, anthropic) = auto_parse_json(
        br#"{"message":{"model":"deepseek-chat","usage":{"input_tokens":100,"output_tokens":25}}}"#,
    )
    .unwrap();
    assert_eq!(protocol, Provider::Anthropic);
    assert_eq!(anthropic.tokens.input, 100);
    assert_eq!(anthropic.tokens.output, 25);
}

#[test]
fn automatically_detects_anthropic_sse_completion() {
    let mut parser = AutoStreamUsageParser::new();
    assert!(parser.push(b"event: message_start\ndata: {\"message\":{\"model\":\"deepseek-chat\",\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\n").is_none());
    assert!(parser.push(b"event: message_delta\ndata: {\"usage\":{\"input_tokens\":100,\"output_tokens\":25}}\n\n").is_none());
    let (_, usage) = parser
        .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
        .unwrap();
    assert_eq!(usage.model.as_deref(), Some("deepseek-chat"));
    assert_eq!(usage.tokens.output, 25);
}

#[test]
fn processes_all_completed_sse_events_in_a_single_body_chunk() {
    let mut parser = AutoStreamUsageParser::new();
    let (_, usage) = parser
        .push(
            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5-mini\",\"usage\":{\"input_tokens\":100,\"output_tokens\":10}}}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5-mini\",\"usage\":{\"input_tokens\":100,\"output_tokens\":20}}}\n\n",
        )
        .unwrap();

    assert_eq!(usage.model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(usage.tokens.output, 20);
}

#[test]
fn retains_the_model_from_an_early_openai_event_without_usage() {
    let mut parser = AutoStreamUsageParser::new();
    assert!(parser
        .push(b"data: {\"model\":\"gpt-5-mini\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n")
        .is_none());

    assert!(
        parser
            .push(b"data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20}}\n\n")
            .is_none()
    );
    let (_, usage) = parser.push(b"data: [DONE]\n\n").unwrap();

    assert_eq!(usage.model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(usage.tokens.output, 20);
}

#[test]
fn accepts_sse_fields_without_spaces_or_with_tabs() {
    let mut parser = AutoStreamUsageParser::new();
    assert!(parser
        .push(b"data:{\"model\":\"gpt-5-mini\",\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20}}\n\n")
        .is_none());

    let (_, usage) = parser.push(b"data:\t[DONE]\n\n").unwrap();

    assert_eq!(usage.model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(usage.tokens.output, 20);
}

#[test]
fn ignores_an_empty_sse_event() {
    let mut parser = AutoStreamUsageParser::new();
    assert!(parser.push(b"\n\n").is_none());
    assert!(
        parser
            .push(b"data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20}}\n\n")
            .is_none()
    );
    assert!(parser.push(b"data: [DONE]\n\n").is_some());
}

#[test]
fn parses_a_large_sse_event_split_across_tiny_transport_chunks() {
    let mut parser = AutoStreamUsageParser::new();
    let event = format!(
        "data: {{\"model\":\"gpt-5-mini\",\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
        "x".repeat(128 * 1024)
    );
    for byte in event.as_bytes() {
        assert!(parser.push(std::slice::from_ref(byte)).is_none());
    }
    assert!(
        parser
            .push(b"data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20}}\n\n")
            .is_none()
    );

    let (_, usage) = parser.push(b"data:[DONE]\n\n").unwrap();
    assert_eq!(usage.model.as_deref(), Some("gpt-5-mini"));
    assert_eq!(usage.tokens.output, 20);
}

#[test]
fn parses_cursor_connect_usage_for_any_requested_model() {
    let mut parser = CursorUsageParser::new();
    let model = "glm-4.7";
    let model_details = protobuf_length_field(1, model.as_bytes());
    let run_request = protobuf_length_field(3, &model_details);
    let client_message = protobuf_length_field(1, &run_request);
    assert_eq!(
        parser.push_request(&connect_frame(&client_message)),
        Some(model.to_owned())
    );
    assert!(!parser.request_reported());
    parser.mark_request_reported();
    assert!(parser.request_reported());

    let token_delta = protobuf_varint_field(1, 37);
    let interaction_update = protobuf_length_field(8, &token_delta);
    let server_message = protobuf_length_field(1, &interaction_update);
    let usage = parser
        .push_response(&connect_frame(&server_message))
        .unwrap();
    assert_eq!(usage.model.as_deref(), Some(model));
    assert_eq!(usage.tokens.output, 37);

    let turn_ended = protobuf_length_field(14, &[]);
    let server_message = protobuf_length_field(1, &turn_ended);
    assert!(
        parser
            .push_response(&connect_frame(&server_message))
            .is_none()
    );
}

#[test]
fn parses_deepseek_anthropic_compatible_streams_used_by_claude_code() {
    let mut parser = StreamUsageParser::new(Provider::DeepSeek);
    assert!(parser.push(b"event: message_start\ndata: {\"message\":{\"model\":\"deepseek-chat\",\"usage\":{\"input_tokens\":100,\"output_tokens\":1}}}\n\n").is_none());
    assert!(parser.push(b"event: message_delta\ndata: {\"usage\":{\"input_tokens\":100,\"output_tokens\":25,\"cache_read_input_tokens\":40}}\n\n").is_none());

    let usage = parser
        .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
        .unwrap();

    assert_eq!(usage.model.as_deref(), Some("deepseek-chat"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 100,
            output: 25,
            cache_read: 40,
            cache_write: 0,
        }
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
fn emits_gemini_usage_from_the_final_sse_event() {
    let mut parser = StreamUsageParser::new(Provider::Gemini);
    let usage = parser
        .push(b"data: {\"modelVersion\":\"gemini-2.5-flash\",\"usageMetadata\":{\"promptTokenCount\":120,\"cachedContentTokenCount\":20,\"totalTokenCount\":165}}\n\n")
        .unwrap();
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
fn parses_cloud_code_assist_gemini_usage_wrapped_in_response() {
    let usage = Provider::Gemini
        .parse_json(
            br#"{"response":{"modelVersion":"gemini-3.7-flash","usageMetadata":{"promptTokenCount":120,"cachedContentTokenCount":20,"totalTokenCount":165}}}"#,
        )
        .unwrap();

    assert_eq!(usage.model.as_deref(), Some("gemini-3.7-flash"));
    assert_eq!(
        usage.tokens,
        TokenUsage {
            input: 100,
            output: 45,
            cache_read: 20,
            cache_write: 0,
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
    let mut parser = WebSocketUsageParser::new();

    assert!(parser.push(&frame[..5]).is_none());
    let (protocol, usage) = parser.push(&frame[5..]).unwrap();

    assert_eq!(protocol, Provider::OpenAiResponses);
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
    let mut parser = WebSocketUsageParser::with_permessage_deflate(false);

    let (protocol, usage) = parser.push(&frame).unwrap();

    assert_eq!(protocol, Provider::OpenAiResponses);
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
    let mut parser = WebSocketUsageParser::with_permessage_deflate(false);

    assert_eq!(parser.push(&first).unwrap().1.tokens.input, 100);
    let (_, usage) = parser.push(&second).unwrap();

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

fn connect_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
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
