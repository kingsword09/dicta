# Changelog

## [v0.6.0](https://github.com/k1LoW/vo/compare/v0.5.1...v0.6.0) - 2026-06-16

### New Features 🎉
- feat: select mic / speaker device interactively at startup by @k1LoW in https://github.com/k1LoW/vo/pull/23
- feat: follow the system default device instead of stopping on change by @k1LoW in https://github.com/k1LoW/vo/pull/24
- feat: show the capture device for each channel in the startup banner by @k1LoW in https://github.com/k1LoW/vo/pull/26
### Other Changes
- docs: add a Troubleshooting section to the README by @k1LoW in https://github.com/k1LoW/vo/pull/27

## [v0.5.1](https://github.com/k1LoW/vo/compare/v0.5.0...v0.5.1) - 2026-06-12

### Other Changes
- fix: drop float artifacts from JSONL confidence and audio offsets by @k1LoW in https://github.com/k1LoW/vo/pull/21

## [v0.5.0](https://github.com/k1LoW/vo/compare/v0.4.0...v0.5.0) - 2026-06-12

### New Features 🎉
- feat: emit per-chunk transcription confidence under src.confidence by @k1LoW in https://github.com/k1LoW/vo/pull/19
- feat: stop gracefully when the audio device changes mid-session by @k1LoW in https://github.com/k1LoW/vo/pull/20

## [v0.4.0](https://github.com/k1LoW/vo/compare/v0.3.0...v0.4.0) - 2026-06-11

### New Features 🎉
- feat: attribute TCC permissions to vo, not the launching terminal by @k1LoW in https://github.com/k1LoW/vo/pull/16

## [v0.3.0](https://github.com/k1LoW/vo/compare/v0.2.1...v0.3.0) - 2026-06-11

### New Features 🎉
- feat: use a Core Audio tap for speaker capture to drop Screen Recording by @k1LoW in https://github.com/k1LoW/vo/pull/14
- feat: resolve speech and translation models up front at startup by @k1LoW in https://github.com/k1LoW/vo/pull/15
### Other Changes
- test: add Swift Testing suite and CI by @k1LoW in https://github.com/k1LoW/vo/pull/11
- fix: drop script subtag from --doctor translation list by @k1LoW in https://github.com/k1LoW/vo/pull/13

## [v0.2.1](https://github.com/k1LoW/vo/compare/v0.2.0...v0.2.1) - 2026-06-10

### Other Changes
- fix: correctness and stability bugs surfaced by live runs by @k1LoW in https://github.com/k1LoW/vo/pull/10

## [v0.2.0](https://github.com/k1LoW/vo/compare/v0.1.1...v0.2.0) - 2026-06-10

### New Features 🎉
- feat: add --transcript option with exit-time save prompt by @k1LoW in https://github.com/k1LoW/vo/pull/4
- feat: lower live-caption latency via fastResults + prepareToAnalyze by @k1LoW in https://github.com/k1LoW/vo/pull/6
### Other Changes
- docs: document manual install steps and fix releases URL by @k1LoW in https://github.com/k1LoW/vo/pull/7
- feat: quiet down channel labels and tint timestamps by channel by @k1LoW in https://github.com/k1LoW/vo/pull/8

## [v0.1.1](https://github.com/k1LoW/vo/compare/v0.1.0...v0.1.1) - 2026-06-10

### Dependency Updates ⬆️
- chore(deps): bump actions/checkout from 4.3.1 to 6.0.3 in the dependencies group across 1 directory by @dependabot[bot] in https://github.com/k1LoW/vo/pull/2

## [v0.1.0](https://github.com/k1LoW/vo/commits/v0.1.0) - 2026-06-10
