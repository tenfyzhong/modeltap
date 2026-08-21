use crate::config::LoggingConfig;
use crate::usage::TokenUsage;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

pub const BODY_PREVIEW_LIMIT: usize = 4 * 1024;

pub fn init(config: &LoggingConfig) -> io::Result<()> {
    let writer = LogWriter::open(config.file.clone())?;
    let filter = EnvFilter::new(config.level.as_str());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .with_writer(move || writer.clone())
        .try_init();
    Ok(())
}

#[derive(Clone)]
struct LogWriter {
    file: Option<Arc<Mutex<File>>>,
}

impl LogWriter {
    fn open(path: Option<PathBuf>) -> io::Result<Self> {
        let file = path
            .map(|path| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map(|file| Arc::new(Mutex::new(file)))
            })
            .transpose()?;
        Ok(Self { file })
    }
}

impl Write for LogWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        io::stderr().write_all(buffer)?;
        if let Some(file) = &self.file {
            file.lock()
                .map_err(|_| io::Error::other("log file lock poisoned"))?
                .write_all(buffer)?;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()?;
        if let Some(file) = &self.file {
            file.lock()
                .map_err(|_| io::Error::other("log file lock poisoned"))?
                .flush()?;
        }
        Ok(())
    }
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

pub fn usage_report_summary(
    site: &str,
    model: &str,
    agent_cli: &str,
    usage: &TokenUsage,
) -> String {
    format!(
        "site={site} model={model} agent_cli={agent_cli} input_tokens={} output_tokens={} cache_read_tokens={} cache_write_tokens={}",
        usage.input, usage.output, usage.cache_read, usage.cache_write
    )
}

#[cfg(test)]
mod tests {
    use super::LogWriter;
    use std::io::Write;

    #[test]
    fn log_writer_appends_to_the_configured_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("modeltap.log");
        let mut writer = LogWriter::open(Some(path.clone())).unwrap();

        writer.write_all(b"recorded log line\n").unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "recorded log line\n"
        );
    }
}
