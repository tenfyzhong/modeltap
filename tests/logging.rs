use modeltap::logging::{body_preview, usage_report_summary};
use modeltap::usage::TokenUsage;

#[test]
fn body_previews_are_bounded_and_identify_truncation() {
    let body = b"abcdefghijklmnopqrstuvwxyz";
    let preview = body_preview(body, 8);

    assert_eq!(preview, "abcdefgh… (18 bytes omitted)");
}

#[test]
fn body_previews_preserve_complete_utf8_content() {
    assert_eq!(body_preview("hello 世界".as_bytes(), 64), "hello 世界");
}

#[test]
fn usage_report_summaries_include_every_reported_token_category() {
    let summary = usage_report_summary(
        "openai",
        "gpt-5.6-terra",
        "oh_my_pi",
        &TokenUsage {
            input: 70,
            output: 20,
            cache_read: 30,
            cache_write: 4,
        },
    );

    assert_eq!(
        summary,
        "site=openai model=gpt-5.6-terra agent_cli=oh_my_pi input_tokens=70 output_tokens=20 cache_read_tokens=30 cache_write_tokens=4"
    );
}
