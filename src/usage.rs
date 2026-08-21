use flate2::{Decompress, FlushDecompress};
use serde::Deserialize;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUsage {
    pub model: Option<String>,
    pub tokens: TokenUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAiChat,
    OpenAiResponses,
    OpenAiEmbedding,
    Anthropic,
    DeepSeek,
    Gemini,
    Cursor,
}

#[derive(Debug, Deserialize)]
struct UsageDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct UsagePayload {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<UsageDetails>,
    #[serde(default)]
    input_tokens_details: Option<UsageDetails>,
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: Option<u64>,
    #[serde(rename = "cachedContentTokenCount", default)]
    cached_content_token_count: Option<u64>,
    #[serde(rename = "totalTokenCount", default)]
    total_token_count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct UsageEvent {
    #[serde(default)]
    model: Option<String>,
    #[serde(rename = "modelVersion", default)]
    model_version: Option<String>,
    #[serde(default)]
    usage: Option<UsagePayload>,
    #[serde(rename = "usageMetadata", default)]
    usage_metadata: Option<UsagePayload>,
    #[serde(default)]
    response: Option<Box<UsageEvent>>,
    #[serde(default)]
    message: Option<Box<UsageEvent>>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

pub fn permessage_deflate_server_no_context_takeover(value: &str) -> Option<bool> {
    value.split(',').find_map(|extension| {
        let mut parts = extension.split(';');
        parts
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("permessage-deflate"))
            .then(|| {
                parts.any(|parameter| {
                    parameter
                        .trim()
                        .eq_ignore_ascii_case("server_no_context_takeover")
                })
            })
    })
}

impl Provider {
    pub fn from_config(value: &str) -> Option<Self> {
        match value {
            "openai" | "openai_chat" => Some(Self::OpenAiChat),
            "openai_responses" => Some(Self::OpenAiResponses),
            "openai_embeddings" => Some(Self::OpenAiEmbedding),
            "anthropic" => Some(Self::Anthropic),
            "deepseek" => Some(Self::DeepSeek),
            "gemini" => Some(Self::Gemini),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    pub fn parse_json(self, bytes: &[u8]) -> Option<ParsedUsage> {
        let value: UsageEvent = serde_json::from_slice(bytes).ok()?;
        self.parse_value(&value)
    }

    fn parse_value(self, value: &UsageEvent) -> Option<ParsedUsage> {
        match self {
            Self::OpenAiChat | Self::OpenAiEmbedding => parse_openai(value),
            Self::OpenAiResponses => parse_openai(value.response.as_deref().unwrap_or(value)),
            Self::Anthropic => parse_anthropic(value),
            Self::DeepSeek => parse_deepseek(value),
            Self::Gemini => parse_gemini(value),
            Self::Cursor => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai",
            Self::OpenAiResponses => "openai_responses",
            Self::OpenAiEmbedding => "openai_embeddings",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor",
        }
    }
}

pub fn auto_parse_json(bytes: &[u8]) -> Option<(Provider, ParsedUsage)> {
    let value: UsageEvent = serde_json::from_slice(bytes).ok()?;
    auto_parse_value(&value)
}

fn auto_parse_value(value: &UsageEvent) -> Option<(Provider, ParsedUsage)> {
    let provider = if value.usage_metadata.is_some()
        || value
            .response
            .as_deref()
            .is_some_and(|response| response.usage_metadata.is_some())
    {
        Provider::Gemini
    } else if value.response.is_some()
        || value
            .kind
            .as_deref()
            .is_some_and(|kind| kind.starts_with("response."))
    {
        Provider::OpenAiResponses
    } else if value.message.is_some()
        || value
            .kind
            .as_deref()
            .is_some_and(|kind| kind.starts_with("message_"))
        || value
            .usage
            .as_ref()
            .is_some_and(|usage| usage.input_tokens.is_some())
    {
        Provider::Anthropic
    } else if value
        .usage
        .as_ref()
        .is_some_and(|usage| usage.prompt_tokens.is_some())
    {
        Provider::OpenAiChat
    } else {
        return None;
    };
    provider.parse_value(value).map(|usage| (provider, usage))
}

fn model_from_value(value: &UsageEvent) -> Option<String> {
    [
        value.model.as_deref(),
        value.model_version.as_deref(),
        value
            .response
            .as_deref()
            .and_then(|response| response.model.as_deref()),
        value
            .response
            .as_deref()
            .and_then(|response| response.model_version.as_deref()),
        value
            .message
            .as_deref()
            .and_then(|message| message.model.as_deref()),
    ]
    .into_iter()
    .flatten()
    .find(|model| !model.is_empty())
    .map(ToOwned::to_owned)
}

const MAX_CURSOR_CONNECT_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Parses Cursor's Connect/Protobuf agent stream without relying on a model allowlist.
pub struct CursorUsageParser {
    request_buffer: Vec<u8>,
    response_buffer: Vec<u8>,
    model: Option<String>,
    request_reported: bool,
}

impl CursorUsageParser {
    pub fn new() -> Self {
        Self {
            request_buffer: Vec::new(),
            response_buffer: Vec::new(),
            model: None,
            request_reported: false,
        }
    }

    pub fn push_request(&mut self, bytes: &[u8]) -> Option<String> {
        self.request_buffer.extend_from_slice(bytes);
        while let Some(message) = take_connect_message(&mut self.request_buffer) {
            if let Some(model) = cursor_model_from_request(&message) {
                self.model = Some(model.clone());
                return Some(model);
            }
        }
        None
    }

    pub fn push_response(&mut self, bytes: &[u8]) -> Option<ParsedUsage> {
        self.response_buffer.extend_from_slice(bytes);
        while let Some(message) = take_connect_message(&mut self.response_buffer) {
            let Some(interaction) = protobuf_length_field(&message, 1) else {
                continue;
            };
            if let Some(token_delta) = protobuf_length_field(interaction, 8)
                .and_then(|value| protobuf_varint_field(value, 1))
            {
                return Some(ParsedUsage {
                    model: self.model.clone(),
                    tokens: TokenUsage {
                        output: token_delta,
                        ..TokenUsage::default()
                    },
                });
            }
        }
        None
    }

    pub fn request_reported(&self) -> bool {
        self.request_reported
    }

    pub fn mark_request_reported(&mut self) {
        self.request_reported = true;
    }
}

pub struct StreamUsageParser {
    provider: Provider,
    buffer: Vec<u8>,
    latest: Option<ParsedUsage>,
}

pub struct AutoStreamUsageParser {
    buffer: Vec<u8>,
    consumed: usize,
    scan_from: usize,
    model: Option<String>,
    latest: Option<(Provider, ParsedUsage)>,
}

const BUFFER_COMPACTION_THRESHOLD: usize = 64 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

impl AutoStreamUsageParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            consumed: 0,
            scan_from: 0,
            model: None,
            latest: None,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Option<(Provider, ParsedUsage)> {
        self.buffer.extend_from_slice(bytes);
        let mut result = None;
        while let Some(event_end) = find_event_end_from(&self.buffer, self.scan_from) {
            let separator =
                if event_end >= 4 && self.buffer[event_end - 4..event_end] == *b"\r\n\r\n" {
                    4
                } else {
                    2
                };
            let parsed = parse_sse_event(
                &self.buffer[self.consumed..event_end - separator],
                self.model.is_none(),
            );
            self.consumed = event_end;
            self.scan_from = event_end;
            if let Some(usage) = self.process_event(parsed) {
                result = Some(usage);
            }
        }
        if self.buffer.len() - self.consumed > MAX_SSE_EVENT_BYTES {
            self.buffer.clear();
            self.consumed = 0;
            self.scan_from = 0;
            return result;
        }
        self.scan_from = self.buffer.len().saturating_sub(3).max(self.consumed);
        self.compact_buffer();
        result
    }

    fn compact_buffer(&mut self) {
        if self.consumed == self.buffer.len() {
            self.buffer.clear();
            self.consumed = 0;
            self.scan_from = 0;
        } else if self.consumed >= BUFFER_COMPACTION_THRESHOLD {
            self.buffer.copy_within(self.consumed.., 0);
            self.buffer.truncate(self.buffer.len() - self.consumed);
            self.scan_from = self.scan_from.saturating_sub(self.consumed);
            self.consumed = 0;
        }
    }

    fn process_event(&mut self, event: SseEvent) -> Option<(Provider, ParsedUsage)> {
        if event.is_done {
            return self.latest.clone();
        }

        if let Some(value) = event.value {
            if self.model.is_none() {
                self.model = model_from_value(&value);
            }
            if let Some((provider, mut usage)) = auto_parse_value(&value) {
                if usage.model.is_none() {
                    usage.model = self.model.clone().or_else(|| {
                        self.latest
                            .as_ref()
                            .and_then(|(_, usage)| usage.model.clone())
                    });
                }
                if self.model.is_none() {
                    self.model = usage.model.clone();
                }
                self.latest = Some((provider, usage));
            }
        }
        event
            .is_terminal
            .then(|| self.latest.clone())
            .flatten()
            .or_else(|| {
                self.latest
                    .as_ref()
                    .filter(|(provider, _)| *provider == Provider::Gemini)
                    .cloned()
            })
    }
}

struct SseEvent {
    is_done: bool,
    is_terminal: bool,
    value: Option<UsageEvent>,
}

fn parse_sse_event(event: &[u8], needs_model: bool) -> SseEvent {
    let mut event_name = &[][..];
    let mut data = None;
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"event:") {
            event_name = trim_ascii(value);
        } else if let Some(value) = line.strip_prefix(b"data:") {
            append_sse_data(&mut data, trim_ascii_start(value));
        }
    }
    let data = data.unwrap_or(Cow::Borrowed(&[]));
    let is_done = trim_ascii(&data) == b"[DONE]";
    let is_terminal = matches!(event_name, b"message_stop" | b"response.completed");
    let (has_usage, has_model) = json_fields_of_interest(&data, needs_model);
    let value = (has_usage || has_model)
        .then(|| serde_json::from_slice(&data).ok())
        .flatten();
    SseEvent {
        is_done,
        is_terminal,
        value,
    }
}

fn append_sse_data<'a>(data: &mut Option<Cow<'a, [u8]>>, value: &'a [u8]) {
    match data.take() {
        None => *data = Some(Cow::Borrowed(value)),
        Some(Cow::Borrowed(previous)) => {
            let mut joined = Vec::with_capacity(previous.len() + value.len() + 1);
            joined.extend_from_slice(previous);
            joined.push(b'\n');
            joined.extend_from_slice(value);
            *data = Some(Cow::Owned(joined));
        }
        Some(Cow::Owned(mut joined)) => {
            joined.push(b'\n');
            joined.extend_from_slice(value);
            *data = Some(Cow::Owned(joined));
        }
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

fn trim_ascii_start(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii_start()
}

fn json_fields_of_interest(bytes: &[u8], needs_model: bool) -> (bool, bool) {
    let mut has_usage = false;
    let mut has_model = false;
    for index in 0..bytes.len() {
        if bytes[index] != b'\"' {
            continue;
        }
        let field = &bytes[index..];
        has_usage |= field.starts_with(b"\"usage\"") || field.starts_with(b"\"usageMetadata\"");
        if needs_model {
            has_model |= field.starts_with(b"\"model\"") || field.starts_with(b"\"modelVersion\"");
        }
        if has_usage && (!needs_model || has_model) {
            break;
        }
    }
    (has_usage, has_model)
}

const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

struct WebSocketFrame {
    length: usize,
    fin: bool,
    opcode: u8,
    payload_start: usize,
    payload_length: usize,
    mask: Option<[u8; 4]>,
    compressed: bool,
}

struct PerMessageDeflate {
    decompressor: Decompress,
    server_no_context_takeover: bool,
}

pub struct WebSocketUsageParser {
    buffer: Vec<u8>,
    fragmented_opcode: Option<u8>,
    fragmented_compressed: bool,
    fragments: Vec<u8>,
    sse: AutoStreamUsageParser,
    permessage_deflate: Option<PerMessageDeflate>,
}

impl WebSocketUsageParser {
    pub fn new() -> Self {
        Self::build(None)
    }

    pub fn with_permessage_deflate(server_no_context_takeover: bool) -> Self {
        Self::build(Some(server_no_context_takeover))
    }

    fn build(server_no_context_takeover: Option<bool>) -> Self {
        Self {
            buffer: Vec::new(),
            fragmented_opcode: None,
            fragmented_compressed: false,
            fragments: Vec::new(),
            sse: AutoStreamUsageParser::new(),
            permessage_deflate: server_no_context_takeover.map(|server_no_context_takeover| {
                PerMessageDeflate {
                    decompressor: Decompress::new(false),
                    server_no_context_takeover,
                }
            }),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Option<(Provider, ParsedUsage)> {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.extend_from_slice(bytes);
        let mut consumed = 0;
        while let Some(frame) = websocket_frame(&buffer[consumed..]) {
            let payload_start = consumed + frame.payload_start;
            let payload_end = payload_start + frame.payload_length;
            consumed += frame.length;
            let usage = if let Some(mask) = frame.mask {
                let mut payload = buffer[payload_start..payload_end].to_vec();
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % mask.len()];
                }
                self.process_frame(frame.fin, frame.opcode, frame.compressed, &payload)
            } else {
                self.process_frame(
                    frame.fin,
                    frame.opcode,
                    frame.compressed,
                    &buffer[payload_start..payload_end],
                )
            };
            if let Some(usage) = usage {
                compact_websocket_buffer(&mut buffer, consumed);
                self.buffer = buffer;
                return Some(usage);
            }
        }
        compact_websocket_buffer(&mut buffer, consumed);
        self.buffer = buffer;
        None
    }

    fn process_frame(
        &mut self,
        fin: bool,
        opcode: u8,
        compressed: bool,
        payload: &[u8],
    ) -> Option<(Provider, ParsedUsage)> {
        match opcode {
            0x1 => {
                if fin {
                    self.process_message(payload, compressed)
                } else {
                    self.fragmented_opcode = Some(opcode);
                    self.fragmented_compressed = compressed;
                    self.fragments.clear();
                    self.extend_fragments(payload);
                    None
                }
            }
            0x0 if self.fragmented_opcode == Some(0x1) => {
                self.extend_fragments(payload);
                if fin {
                    self.fragmented_opcode = None;
                    let payload = std::mem::take(&mut self.fragments);
                    let compressed = std::mem::take(&mut self.fragmented_compressed);
                    self.process_message(&payload, compressed)
                } else {
                    None
                }
            }
            0x8 => {
                self.fragmented_opcode = None;
                self.fragmented_compressed = false;
                self.fragments.clear();
                None
            }
            _ => None,
        }
    }

    fn process_message(
        &mut self,
        payload: &[u8],
        compressed: bool,
    ) -> Option<(Provider, ParsedUsage)> {
        if !compressed {
            return self.process_text(payload);
        }
        let payload = self.decompress_message(payload)?;
        self.process_text(&payload)
    }

    fn decompress_message(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        let compression = self.permessage_deflate.as_mut()?;
        let mut input = Vec::with_capacity(payload.len() + 4);
        input.extend_from_slice(payload);
        input.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
        let mut output = Vec::with_capacity(payload.len().saturating_mul(2).max(1024));
        let mut consumed = 0;
        while consumed < input.len() {
            if output.len() >= MAX_WEBSOCKET_MESSAGE_BYTES {
                return None;
            }
            output.reserve(16 * 1024);
            let input_before = compression.decompressor.total_in();
            let output_before = compression.decompressor.total_out();
            compression
                .decompressor
                .decompress_vec(&input[consumed..], &mut output, FlushDecompress::Sync)
                .ok()?;
            let input_progress = (compression.decompressor.total_in() - input_before) as usize;
            let output_progress = (compression.decompressor.total_out() - output_before) as usize;
            if input_progress == 0 && output_progress == 0 {
                return None;
            }
            consumed = consumed.saturating_add(input_progress);
        }
        if output.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return None;
        }
        if compression.server_no_context_takeover {
            compression.decompressor.reset(false);
        }
        Some(output)
    }

    fn extend_fragments(&mut self, payload: &[u8]) {
        if self.fragments.len().saturating_add(payload.len()) > MAX_WEBSOCKET_MESSAGE_BYTES {
            self.fragmented_opcode = None;
            self.fragments.clear();
            return;
        }
        self.fragments.extend_from_slice(payload);
    }

    fn process_text(&mut self, payload: &[u8]) -> Option<(Provider, ParsedUsage)> {
        if let Some(usage) = auto_parse_json(payload) {
            return Some(usage);
        }
        self.sse.push(payload)
    }
}

fn compact_websocket_buffer(buffer: &mut Vec<u8>, consumed: usize) {
    if consumed == buffer.len() {
        buffer.clear();
    } else if consumed > 0 {
        buffer.copy_within(consumed.., 0);
        buffer.truncate(buffer.len() - consumed);
    }
}

impl StreamUsageParser {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            buffer: Vec::new(),
            latest: None,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Option<ParsedUsage> {
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = find_event_end(&self.buffer) {
            let event = self.buffer.drain(..end).collect::<Vec<_>>();
            let separator = if event.ends_with(b"\r\n\r\n") { 4 } else { 2 };
            let event = &event[..event.len() - separator];
            if let Some(usage) = self.process_event(event) {
                return Some(usage);
            }
        }
        None
    }

    pub fn finish(self) -> Option<ParsedUsage> {
        match self.provider {
            Provider::Gemini => self.latest,
            _ => None,
        }
    }

    fn process_event(&mut self, event: &[u8]) -> Option<ParsedUsage> {
        let Ok(event) = std::str::from_utf8(event) else {
            return None;
        };
        let mut event_name = "";
        let mut data = String::new();
        for line in event.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("event:") {
                event_name = value.trim();
            } else if let Some(value) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value.trim_start());
            }
        }
        if data == "[DONE]" {
            return matches!(self.provider, Provider::OpenAiChat | Provider::DeepSeek)
                .then(|| self.latest.clone())
                .flatten();
        }
        let parsed = self.provider.parse_json(data.as_bytes());
        if let Some(parsed) = parsed {
            self.update(parsed);
        }
        match self.provider {
            Provider::OpenAiResponses if event_name == "response.completed" => self.latest.clone(),
            Provider::Anthropic | Provider::DeepSeek if event_name == "message_stop" => {
                self.latest.clone()
            }
            Provider::Gemini => self.latest.clone(),
            _ => None,
        }
    }

    fn update(&mut self, mut parsed: ParsedUsage) {
        if parsed.model.is_none() {
            parsed.model = self.latest.as_ref().and_then(|usage| usage.model.clone());
        }
        self.latest = Some(parsed);
    }
}

fn find_event_end(bytes: &[u8]) -> Option<usize> {
    find_event_end_from(bytes, 0)
}

fn find_event_end_from(bytes: &[u8], start: usize) -> Option<usize> {
    let start = start.min(bytes.len().saturating_sub(1));
    for index in start..bytes.len().saturating_sub(1) {
        if bytes[index] == b'\n' && bytes[index + 1] == b'\n' {
            return Some(index + 2);
        }
        if index + 3 < bytes.len() && bytes[index..index + 4] == *b"\r\n\r\n" {
            return Some(index + 4);
        }
    }
    None
}

fn take_connect_message(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buffer.len() < 5 {
        return None;
    }
    let length = u32::from_be_bytes(buffer[1..5].try_into().ok()?) as usize;
    if length > MAX_CURSOR_CONNECT_MESSAGE_BYTES {
        buffer.clear();
        return None;
    }
    let end = 5usize.checked_add(length)?;
    if buffer.len() < end {
        return None;
    }
    buffer.drain(..5);
    Some(buffer.drain(..length).collect())
}

fn cursor_model_from_request(message: &[u8]) -> Option<String> {
    let run_request = protobuf_length_field(message, 1)?;
    let model_details =
        protobuf_length_field(run_request, 3).or_else(|| protobuf_length_field(run_request, 9))?;
    std::str::from_utf8(protobuf_length_field(model_details, 1)?)
        .ok()
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

fn protobuf_length_field<'a>(message: &'a [u8], wanted_field: u64) -> Option<&'a [u8]> {
    let mut cursor = 0;
    while cursor < message.len() {
        let key = protobuf_varint(message, &mut cursor)?;
        let field = key >> 3;
        match key & 0x07 {
            0 => {
                let _ = protobuf_varint(message, &mut cursor)?;
            }
            1 => cursor = cursor.checked_add(8)?,
            2 => {
                let length = usize::try_from(protobuf_varint(message, &mut cursor)?).ok()?;
                let end = cursor.checked_add(length)?;
                if end > message.len() {
                    return None;
                }
                if field == wanted_field {
                    return Some(&message[cursor..end]);
                }
                cursor = end;
            }
            5 => cursor = cursor.checked_add(4)?,
            _ => return None,
        }
        if cursor > message.len() {
            return None;
        }
    }
    None
}

fn protobuf_varint_field(message: &[u8], wanted_field: u64) -> Option<u64> {
    let mut cursor = 0;
    while cursor < message.len() {
        let key = protobuf_varint(message, &mut cursor)?;
        let field = key >> 3;
        match key & 0x07 {
            0 => {
                let value = protobuf_varint(message, &mut cursor)?;
                if field == wanted_field {
                    return Some(value);
                }
            }
            1 => cursor = cursor.checked_add(8)?,
            2 => {
                let length = usize::try_from(protobuf_varint(message, &mut cursor)?).ok()?;
                cursor = cursor.checked_add(length)?;
            }
            5 => cursor = cursor.checked_add(4)?,
            _ => return None,
        }
        if cursor > message.len() {
            return None;
        }
    }
    None
}

fn protobuf_varint(message: &[u8], cursor: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = *message.get(*cursor)?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn websocket_frame(bytes: &[u8]) -> Option<WebSocketFrame> {
    if bytes.len() < 2 {
        return None;
    }
    let fin = bytes[0] & 0x80 != 0;
    let compressed = bytes[0] & 0x40 != 0;
    let opcode = bytes[0] & 0x0f;
    let masked = bytes[1] & 0x80 != 0;
    let mut payload_length = (bytes[1] & 0x7f) as usize;
    let mut cursor = 2;
    if payload_length == 126 {
        if bytes.len() < cursor + 2 {
            return None;
        }
        payload_length = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        cursor += 2;
    } else if payload_length == 127 {
        if bytes.len() < cursor + 8 {
            return None;
        }
        let length = u64::from_be_bytes(bytes[cursor..cursor + 8].try_into().ok()?);
        payload_length = usize::try_from(length).ok()?;
        cursor += 8;
    }
    if payload_length > MAX_WEBSOCKET_MESSAGE_BYTES {
        return None;
    }
    let mask = if masked {
        if bytes.len() < cursor + 4 {
            return None;
        }
        let mask = bytes[cursor..cursor + 4].try_into().ok()?;
        cursor += 4;
        Some(mask)
    } else {
        None
    };
    let frame_length = cursor.checked_add(payload_length)?;
    (bytes.len() >= frame_length).then_some(WebSocketFrame {
        length: frame_length,
        fin,
        opcode,
        payload_start: cursor,
        payload_length,
        mask,
        compressed,
    })
}

fn parse_openai(value: &UsageEvent) -> Option<ParsedUsage> {
    let usage = value.usage.as_ref()?;
    let input_total = usage.prompt_tokens.or(usage.input_tokens)?;
    let output = usage.completion_tokens.or(usage.output_tokens).unwrap_or(0);
    let details = usage
        .prompt_tokens_details
        .as_ref()
        .or(usage.input_tokens_details.as_ref());
    let cache_read = details.and_then(|value| value.cached_tokens).unwrap_or(0);
    let cache_write = details
        .and_then(|value| value.cache_write_tokens)
        .unwrap_or(0);
    Some(ParsedUsage {
        model: value.model.clone(),
        tokens: TokenUsage {
            input: input_total.saturating_sub(cache_read.saturating_add(cache_write)),
            output,
            cache_read,
            cache_write,
        },
    })
}

fn parse_anthropic(value: &UsageEvent) -> Option<ParsedUsage> {
    let usage = value
        .usage
        .as_ref()
        .or_else(|| value.message.as_deref()?.usage.as_ref())?;
    let input = usage.input_tokens?;
    let output = usage.output_tokens.unwrap_or(0);
    let model = value.model.clone().or_else(|| {
        value
            .message
            .as_deref()
            .and_then(|message| message.model.clone())
    });
    Some(ParsedUsage {
        model,
        tokens: TokenUsage {
            input,
            output,
            cache_read: usage.cache_read_input_tokens.unwrap_or(0),
            cache_write: usage.cache_creation_input_tokens.unwrap_or(0),
        },
    })
}

fn parse_deepseek(value: &UsageEvent) -> Option<ParsedUsage> {
    if value.message.is_some() {
        return parse_anthropic(value);
    }
    let usage = value.usage.as_ref()?;
    if usage.cache_read_input_tokens.is_some() || usage.cache_creation_input_tokens.is_some() {
        parse_anthropic(value)
    } else {
        parse_openai(value)
    }
}

fn parse_gemini(value: &UsageEvent) -> Option<ParsedUsage> {
    let value = value.response.as_deref().unwrap_or(value);
    let usage = value.usage_metadata.as_ref()?;
    let prompt = usage.prompt_token_count?;
    let cache_read = usage.cached_content_token_count.unwrap_or(0);
    let total = usage.total_token_count?;
    Some(ParsedUsage {
        model: value.model_version.clone(),
        tokens: TokenUsage {
            input: prompt.saturating_sub(cache_read),
            output: total.saturating_sub(prompt),
            cache_read,
            cache_write: 0,
        },
    })
}
