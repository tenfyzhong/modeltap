use flate2::{Decompress, FlushDecompress};
use serde_json::Value;

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
    Gemini,
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
            "openai" | "openai_chat" | "deepseek" => Some(Self::OpenAiChat),
            "openai_responses" => Some(Self::OpenAiResponses),
            "openai_embeddings" => Some(Self::OpenAiEmbedding),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    pub fn parse_json(self, bytes: &[u8]) -> Option<ParsedUsage> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        match self {
            Self::OpenAiChat | Self::OpenAiEmbedding => parse_openai(&value),
            Self::OpenAiResponses => parse_openai(value.get("response").unwrap_or(&value)),
            Self::Anthropic => parse_anthropic(&value),
            Self::Gemini => parse_gemini(&value),
        }
    }
}

pub struct StreamUsageParser {
    provider: Provider,
    buffer: Vec<u8>,
    latest: Option<ParsedUsage>,
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
    provider: Provider,
    buffer: Vec<u8>,
    fragmented_opcode: Option<u8>,
    fragmented_compressed: bool,
    fragments: Vec<u8>,
    sse: StreamUsageParser,
    permessage_deflate: Option<PerMessageDeflate>,
}

impl WebSocketUsageParser {
    pub fn new(provider: Provider) -> Self {
        Self::build(provider, None)
    }

    pub fn with_permessage_deflate(provider: Provider, server_no_context_takeover: bool) -> Self {
        Self::build(provider, Some(server_no_context_takeover))
    }

    fn build(provider: Provider, server_no_context_takeover: Option<bool>) -> Self {
        let provider = match provider {
            Provider::OpenAiChat => Provider::OpenAiResponses,
            provider => provider,
        };
        Self {
            provider,
            buffer: Vec::new(),
            fragmented_opcode: None,
            fragmented_compressed: false,
            fragments: Vec::new(),
            sse: StreamUsageParser::new(provider),
            permessage_deflate: server_no_context_takeover.map(|server_no_context_takeover| {
                PerMessageDeflate {
                    decompressor: Decompress::new(false),
                    server_no_context_takeover,
                }
            }),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Option<ParsedUsage> {
        self.buffer.extend_from_slice(bytes);
        while let Some(frame) = websocket_frame(&self.buffer) {
            let mut payload = self.buffer
                [frame.payload_start..frame.payload_start + frame.payload_length]
                .to_vec();
            self.buffer.drain(..frame.length);
            if let Some(mask) = frame.mask {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % mask.len()];
                }
            }
            if let Some(usage) =
                self.process_frame(frame.fin, frame.opcode, frame.compressed, &payload)
            {
                return Some(usage);
            }
        }
        None
    }

    fn process_frame(
        &mut self,
        fin: bool,
        opcode: u8,
        compressed: bool,
        payload: &[u8],
    ) -> Option<ParsedUsage> {
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

    fn process_message(&mut self, payload: &[u8], compressed: bool) -> Option<ParsedUsage> {
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

    fn process_text(&mut self, payload: &[u8]) -> Option<ParsedUsage> {
        if let Ok(value) = serde_json::from_slice::<Value>(payload) {
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some("response.completed" | "response.done")
            ) {
                return self.provider.parse_json(payload);
            }
        }
        self.sse.push(payload)
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
            return matches!(self.provider, Provider::OpenAiChat)
                .then(|| self.latest.clone())
                .flatten();
        }
        let parsed = self.provider.parse_json(data.as_bytes());
        if let Some(parsed) = parsed {
            self.update(parsed);
        }
        match self.provider {
            Provider::OpenAiResponses if event_name == "response.completed" => self.latest.clone(),
            Provider::Anthropic if event_name == "message_stop" => self.latest.clone(),
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
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| position + 2)
        })
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

fn parse_openai(value: &Value) -> Option<ParsedUsage> {
    let usage = value.get("usage")?;
    if usage.is_null() {
        return None;
    }
    let input_total = number(usage, "prompt_tokens").or_else(|| number(usage, "input_tokens"))?;
    let output = number(usage, "completion_tokens")
        .or_else(|| number(usage, "output_tokens"))
        .unwrap_or(0);
    let details = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"));
    let cache_read = details
        .and_then(|value| number(value, "cached_tokens"))
        .unwrap_or(0);
    let cache_write = details
        .and_then(|value| number(value, "cache_write_tokens"))
        .unwrap_or(0);
    Some(ParsedUsage {
        model: value
            .get("model")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        tokens: TokenUsage {
            input: input_total.saturating_sub(cache_read.saturating_add(cache_write)),
            output,
            cache_read,
            cache_write,
        },
    })
}

fn parse_anthropic(value: &Value) -> Option<ParsedUsage> {
    let usage = value
        .get("usage")
        .or_else(|| value.get("message")?.get("usage"))?;
    let input = number(usage, "input_tokens")?;
    let output = number(usage, "output_tokens").unwrap_or(0);
    let model = value
        .get("model")
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("model"))
        })
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Some(ParsedUsage {
        model,
        tokens: TokenUsage {
            input,
            output,
            cache_read: number(usage, "cache_read_input_tokens").unwrap_or(0),
            cache_write: number(usage, "cache_creation_input_tokens").unwrap_or(0),
        },
    })
}

fn parse_gemini(value: &Value) -> Option<ParsedUsage> {
    let usage = value.get("usageMetadata")?;
    let prompt = number(usage, "promptTokenCount")?;
    let cache_read = number(usage, "cachedContentTokenCount").unwrap_or(0);
    let total = number(usage, "totalTokenCount")?;
    Some(ParsedUsage {
        model: value
            .get("modelVersion")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        tokens: TokenUsage {
            input: prompt.saturating_sub(cache_read),
            output: total.saturating_sub(prompt),
            cache_read,
            cache_write: 0,
        },
    })
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}
