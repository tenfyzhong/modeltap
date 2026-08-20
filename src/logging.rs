use crate::config::LogLevel;
use crate::usage::TokenUsage;
use tracing_subscriber::EnvFilter;

pub const BODY_PREVIEW_LIMIT: usize = 4 * 1024;

pub fn init(level: LogLevel) {
    let filter = EnvFilter::new(level.as_str());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();
}

pub fn body_preview(body: &[u8], limit: usize) -> String {
    let preview_length = body.len().min(limit);
    let preview = String::from_utf8_lossy(&body[..preview_length]);
    let mut escaped = String::with_capacity(preview.len());
    for character in preview.chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => escaped.extend(control.escape_default()),
            character => escaped.push(character),
        }
    }
    if body.len() > preview_length {
        escaped.push_str(&format!(
            "… ({} bytes omitted)",
            body.len() - preview_length
        ));
    }
    escaped
}

pub fn usage_report_summary(site: &str, provider: &str, model: &str, usage: &TokenUsage) -> String {
    format!(
        "site={site} provider={provider} model={model} input_tokens={} output_tokens={} cache_read_tokens={} cache_write_tokens={}",
        usage.input, usage.output, usage.cache_read, usage.cache_write
    )
}
