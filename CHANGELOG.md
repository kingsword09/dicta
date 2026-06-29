# Changelog

## Unreleased

- Reworked the project into a Rust-first workspace with pluggable ASR providers.
- Replaced the old Doubao Python sidecar path with the Rust `vo-asr-doubao`
  provider.
- Isolated the macOS 26 Apple on-device implementation under
  `legacy/apple-swift-adapter`.
- Moved live CLI rendering, transcript logging, and exit prompts into the Rust
  CLI.
- Added install and release packaging for macOS arm64, Linux x86_64/arm64, and
  Windows x86_64/arm64.

Historical Swift-only releases came from the upstream
[k1LoW/vo](https://github.com/k1LoW/vo) project, which this project thanks and
credits in the README.
