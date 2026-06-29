# Changelog

## [v0.10.2](https://github.com/kingsword09/vo/compare/v0.10.1...v0.10.2) - 2026-06-29

### Other Changes
- fix: improve macos microphone capture fallback by @kingsword09 in https://github.com/kingsword09/vo/pull/11
- feat: add web wasm provider audio storage crate by @kingsword09 in https://github.com/kingsword09/vo/pull/13

## [v0.10.1](https://github.com/kingsword09/vo/compare/v0.10.0...v0.10.1) - 2026-06-29

### Other Changes
- ci: harden cargo dependency fetch in workflows by @kingsword09 in https://github.com/kingsword09/vo/pull/7
- ci: run tagpr after main tests pass by @kingsword09 in https://github.com/kingsword09/vo/pull/9

## [v0.10.0](https://github.com/kingsword09/vo/commits/v0.10.0) - 2026-06-29

### Other Changes
- ci: fix bundled Opus builds with CMake 4 by @kingsword09 in https://github.com/kingsword09/vo/pull/1
- refactor: rename Apple legacy bridge to native adapter by @kingsword09 in https://github.com/kingsword09/vo/pull/2
- fix: make tagpr sync adapter version after Cargo bump by @kingsword09 in https://github.com/kingsword09/vo/pull/4
- chore: customize tagpr release PR template by @kingsword09 in https://github.com/kingsword09/vo/pull/5

## Unreleased

- Reworked the project into a Rust-first workspace with pluggable ASR providers.
- Replaced the old Doubao Python sidecar path with the Rust `vo-asr-doubao`
  provider.
- Isolated the macOS 26 Apple on-device implementation under
  `adapters/apple-speech` and connected it through the native adapter bridge.
- Moved live CLI rendering, transcript logging, and exit prompts into the Rust
  CLI.
- Added install and release packaging for macOS arm64, Linux x86_64/arm64, and
  Windows x86_64/arm64.

Historical Swift-only releases came from the upstream
[k1LoW/vo](https://github.com/k1LoW/vo) project, which this project thanks and
credits in the README.
