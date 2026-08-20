# Contributing to ModelTap

Thank you for contributing. This project is a security-sensitive proxy, so changes must preserve transparent forwarding, safe certificate handling, and low telemetry cardinality.

## Development setup

Install Rust 1.85 or newer, then run:

```bash
make build
make test
make fmt
make check
```

Use a feature or fix branch. Do not commit generated certificates, private keys, Docker credentials, or Grafana Cloud credentials.

## Development rules

- Add or update a reusable test before changing behavior.
- Keep all new documentation and source comments in English.
- Run `cargo fmt`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` before opening a pull request.
- Keep MITM parsing observational: do not buffer complete SSE streams or alter upstream payloads.
- Treat certificate private keys and proxy credentials as secrets. Load credentials from environment variables, mounted files, or a secret manager.
- Avoid unbounded Prometheus or OTLP labels. Request IDs, user IDs, API keys, and full URLs must not become metric labels.

## Pull requests

Describe the user-visible behavior, configuration changes, security implications, and test coverage. If a change affects the public configuration schema, update `README.md`, `docs/index.html`, and the example configuration files in the same pull request.

## Reporting security issues

Do not open a public issue for a vulnerability involving TLS interception, credentials, request content, or telemetry data exposure. Contact the repository maintainers privately through the security contact configured by the repository owner.
