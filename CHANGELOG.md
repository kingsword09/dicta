# Changelog

## [v0.10.28](https://github.com/kingsword09/dicta/compare/v0.10.27...v0.10.28) - 2026-07-12

### Other Changes
- build(release): add size-optimized distribution profile by @kingsword09 in https://github.com/kingsword09/dicta/pull/77

## [v0.10.27](https://github.com/kingsword09/dicta/compare/v0.10.26...v0.10.27) - 2026-07-12

### Other Changes
- fix(cli): bound live redraws and preserve terminal history by @kingsword09 in https://github.com/kingsword09/dicta/pull/75

## [v0.10.26](https://github.com/kingsword09/dicta/compare/v0.10.25...v0.10.26) - 2026-07-12

### Other Changes
- ci: upload a single release archive per platform by @kingsword09 in https://github.com/kingsword09/dicta/pull/73

## [v0.10.25](https://github.com/kingsword09/dicta/compare/v0.10.24...v0.10.25) - 2026-07-12

### Other Changes
- feat(audio): replace RMS endpointing with earshot VAD by @kingsword09 in https://github.com/kingsword09/dicta/pull/71

## [v0.10.24](https://github.com/kingsword09/dicta/compare/v0.10.23...v0.10.24) - 2026-07-10

### Other Changes
- fix(live): cancel stdin audio providers on shutdown by @kingsword09 in https://github.com/kingsword09/dicta/pull/69

## [v0.10.23](https://github.com/kingsword09/dicta/compare/v0.10.22...v0.10.23) - 2026-07-10

### Other Changes
- fix(live): pass provider chunk duration to endpointed audio by @kingsword09 in https://github.com/kingsword09/dicta/pull/67

## [v0.10.22](https://github.com/kingsword09/dicta/compare/v0.10.21...v0.10.22) - 2026-07-10

### Other Changes
- feat(live): add local endpointing for dicta-owned audio by @kingsword09 in https://github.com/kingsword09/dicta/pull/65

## [v0.10.21](https://github.com/kingsword09/dicta/compare/v0.10.20...v0.10.21) - 2026-07-09

### Other Changes
- fix(cli): remember active provider for live and ptt modes by @kingsword09 in https://github.com/kingsword09/dicta/pull/63

## [v0.10.20](https://github.com/kingsword09/dicta/compare/v0.10.19...v0.10.20) - 2026-07-09

### Other Changes
- feat(cli): add dicta-owned audio provider protocol by @kingsword09 in https://github.com/kingsword09/dicta/pull/60

## [v0.10.19](https://github.com/kingsword09/dicta/compare/v0.10.18...v0.10.19) - 2026-07-09

### Other Changes
- feat(audio): add streaming microphone capture API by @kingsword09 in https://github.com/kingsword09/dicta/pull/58

## [v0.10.18](https://github.com/kingsword09/dicta/compare/v0.10.17...v0.10.18) - 2026-07-08

### Other Changes
- feat(ui): unify tray control for live and ptt sessions by @kingsword09 in https://github.com/kingsword09/dicta/pull/56

## [v0.10.17](https://github.com/kingsword09/dicta/compare/v0.10.16...v0.10.17) - 2026-07-08

### Other Changes
- fix(ui): detach tray launcher for ptt switching by @kingsword09 in https://github.com/kingsword09/dicta/pull/54

## [v0.10.16](https://github.com/kingsword09/dicta/compare/v0.10.15...v0.10.16) - 2026-07-08

### Other Changes
- fix(cli): keep ptt transcript prompt cancellable by @kingsword09 in https://github.com/kingsword09/dicta/pull/52

## [v0.10.15](https://github.com/kingsword09/dicta/compare/v0.10.14...v0.10.15) - 2026-07-07

### Other Changes
- fix: gracefully stop live UI worker on shutdown by @kingsword09 in https://github.com/kingsword09/dicta/pull/49
- feat: add realtime activation modes for tray UI by @kingsword09 in https://github.com/kingsword09/dicta/pull/50

## [v0.10.14](https://github.com/kingsword09/dicta/compare/v0.10.13...v0.10.14) - 2026-07-07

### Other Changes
- feat(cli): add external provider ptt mode by @kingsword09 in https://github.com/kingsword09/dicta/pull/47

## [v0.10.13](https://github.com/kingsword09/dicta/compare/v0.10.12...v0.10.13) - 2026-07-07

### Other Changes
- fix: use append-only live rendering for tray mode by @kingsword09 in https://github.com/kingsword09/dicta/pull/45

## [v0.10.12](https://github.com/kingsword09/dicta/compare/v0.10.11...v0.10.12) - 2026-07-07

### Other Changes
- fix: use doctor as a command only by @kingsword09 in https://github.com/kingsword09/dicta/pull/43

## [v0.10.11](https://github.com/kingsword09/dicta/compare/v0.10.10...v0.10.11) - 2026-07-07

### Other Changes
- fix: sign macOS CLI during install by @kingsword09 in https://github.com/kingsword09/dicta/pull/41

## [v0.10.10](https://github.com/kingsword09/dicta/compare/v0.10.9...v0.10.10) - 2026-07-07

### Other Changes
- ci: build macOS release for deployment target 15 by @kingsword09 in https://github.com/kingsword09/dicta/pull/39

## [v0.10.9](https://github.com/kingsword09/dicta/compare/v0.10.8...v0.10.9) - 2026-07-07

### Other Changes
- fix: quiet tray live provider logs by @kingsword09 in https://github.com/kingsword09/dicta/pull/37

## [v0.10.8](https://github.com/kingsword09/dicta/compare/v0.10.7...v0.10.8) - 2026-07-03

### Other Changes
- docs: focus README on OpenAI-compatible provider setup by @kingsword09 in https://github.com/kingsword09/dicta/pull/33
- refactor(cli): move proprietary ASR backends to providers by @kingsword09 in https://github.com/kingsword09/dicta/pull/34
- test(cli): isolate active provider default from platform support by @kingsword09 in https://github.com/kingsword09/dicta/pull/35

## [v0.10.7](https://github.com/kingsword09/dicta/compare/v0.10.6...v0.10.7) - 2026-07-02

### Other Changes
- feat(cli): add provider discovery and update commands by @kingsword09 in https://github.com/kingsword09/dicta/pull/31

## [v0.10.6](https://github.com/kingsword09/dicta/compare/v0.10.5...v0.10.6) - 2026-07-02

### Other Changes
- feat(cli): serve OpenAI-compatible transcription API by @kingsword09 in https://github.com/kingsword09/dicta/pull/28
- refactor: rename project to dicta and add provider install flow by @kingsword09 in https://github.com/kingsword09/dicta/pull/29
