# Changelog

## 0.7.0 (unreleased)

This fork release introduces a coordinated PC/Android protocol update. Install
the matching PC and Android builds together.

### Security

- Protect UDP audio and RTT echo packets with per-session authenticated encryption.
- Authenticate control sessions over certificate-pinned TLS and signed device pairing.
- Restrict updater downloads to this fork's GitHub Releases and verify SHA-256 before
  publishing an APK to the install path.
- Remove runtime ADB dependency; USB transport uses Android USB tethering.

### Maintenance

- Refresh and audit Rust dependencies, including the pinned glib security backport.
- Add a local CI validation runner for hosts where GitHub Actions is unavailable.

## [0.6.0](https://github.com/apirJS/gemacast/compare/v0.5.3...v0.6.0) (2026-09-25)


### Features

* **core:** split audio packet on uncompressed mode to avoid MTU limit ([#96](https://github.com/apirJS/gemacast/issues/96)) ([e1f1992](https://github.com/apirJS/gemacast/commit/e1f19928228a597072c1284c0389c86010abc39e))

## [0.5.3](https://github.com/apirJS/gemacast/compare/v0.5.2...v0.5.3) (2026-09-24)


### Refactoring

* big rewrite ([#94](https://github.com/apirJS/gemacast/issues/94)) ([d4f1360](https://github.com/apirJS/gemacast/commit/d4f13600d33364eccf46d368a5d85956ca36f04a))

## [0.5.2](https://github.com/apirJS/gemacast/compare/v0.5.1...v0.5.2) (2026-09-21)


### Bug Fixes

* **android:** pause output after sustained source silence ([#92](https://github.com/apirJS/gemacast/issues/92)) ([6aac5cf](https://github.com/apirJS/gemacast/commit/6aac5cfbe09c80b1c58b998a0e3006b081dfe0b7))

## [0.5.1](https://github.com/apirJS/gemacast/compare/v0.5.0...v0.5.1) (2026-09-18)


### Bug Fixes

* **mobile:** Windows pause more 60s, android will alway no sound [#87](https://github.com/apirJS/gemacast/issues/87) ([#88](https://github.com/apirJS/gemacast/issues/88)) ([066fbfa](https://github.com/apirJS/gemacast/commit/066fbfad619cf8a32f5f42a42dff824f6ad5c493))

## [0.5.0](https://github.com/apirJS/gemacast/compare/v0.4.0...v0.5.0) (2026-09-06)


### Features

* **mobile:** a toggle to match volume level with pc ([#82](https://github.com/apirJS/gemacast/issues/82)) ([b91db51](https://github.com/apirJS/gemacast/commit/b91db5120715719293ca1d26976edaa16faf7db1))

## [0.4.0](https://github.com/apirJS/gemacast/compare/v0.3.0...v0.4.0) (2026-08-31)


### Features

* **mobile:** simplify setting drawer and auto reconnect ([#80](https://github.com/apirJS/gemacast/issues/80)) ([390ae29](https://github.com/apirJS/gemacast/commit/390ae29686b6e11be60588178d64d08ab18f3663))

## [0.3.0](https://github.com/apirJS/gemacast/compare/v0.2.0...v0.3.0) (2026-08-30)


### Features

* port range migration and firewall warn dialog ([#78](https://github.com/apirJS/gemacast/issues/78)) ([7a83180](https://github.com/apirJS/gemacast/commit/7a831803b2ce32989ed4e327e3ceb67fc50ae6f4))

## [0.2.0](https://github.com/apirJS/gemacast/compare/v0.1.0...v0.2.0) (2026-08-29)


### Features

* **mobile:** remove unused custom media control & fix installation folder issue ([#74](https://github.com/apirJS/gemacast/issues/74)) ([b2600e4](https://github.com/apirJS/gemacast/commit/b2600e45c326e87a6628fac60b4eb3367544cd29))

## [0.2.0](https://github.com/apirJS/gemacast/compare/v0.1.0...v0.2.0) (2026-08-28)


### Features

* **mobile:** remove unused custom media control & fix installation folder issue ([#74](https://github.com/apirJS/gemacast/issues/74)) ([b2600e4](https://github.com/apirJS/gemacast/commit/b2600e45c326e87a6628fac60b4eb3367544cd29))

## 0.1.0 (2026-08-26)


### Features

* a complete review & tweaks before public release ([#66](https://github.com/apirJS/gemacast/issues/66)) ([c2b8005](https://github.com/apirJS/gemacast/commit/c2b80050c777ea46443356f8259180450556951d))
* app update mechanism for all platforms ([d226b30](https://github.com/apirJS/gemacast/commit/d226b30cc08b15512a488568f99bd7eead8e728c))
* app updater with installers, CI hardening ([250f622](https://github.com/apirJS/gemacast/commit/250f622c73ff519f754cb9e9a8a3abc90552105e))
* **audio:** added PLC ([c10eb24](https://github.com/apirJS/gemacast/commit/c10eb246fe00f9e9d99fd88ff3f21d33fc10033b))
* complete review & tweaks before public release ([#67](https://github.com/apirJS/gemacast/issues/67)) ([acc059f](https://github.com/apirJS/gemacast/commit/acc059f4c2138cd87d58d66445e39888dbad6589))
* complete review & tweaks before public release ([#69](https://github.com/apirJS/gemacast/issues/69)) ([7c6c9e1](https://github.com/apirJS/gemacast/commit/7c6c9e15e383f94876ab79dcd23ae6cb1ad7e30d))
* fix stream disconnection bug & improve jitter algorithm ([#64](https://github.com/apirJS/gemacast/issues/64)) ([82ab9aa](https://github.com/apirJS/gemacast/commit/82ab9aa0c6043666f7dc1fc1d1a66c026a786945))
* foreground service, USB tether, static jitter buffer, volume controls ([ea8847b](https://github.com/apirJS/gemacast/commit/ea8847b8b4c8b4aa417cd838219c95062c046756))
* graceful shutdown, connection reliability, tracing ([ae81b0a](https://github.com/apirJS/gemacast/commit/ae81b0a889ace12578d7d77c5afa0fdb27ded18b))
* launch app on startup ([#32](https://github.com/apirJS/gemacast/issues/32)) ([b3ce683](https://github.com/apirJS/gemacast/commit/b3ce683715752dd91d15c5a98ec9841a662f49b1))
* mDNS discovery, gain slider, play/pause stream ([bb7f76e](https://github.com/apirJS/gemacast/commit/bb7f76e41653253b7fa86cd641d8b95ab3158da0))
* Oboe migration, adaptive jitter buffer, bitrate options, preset system ([af14292](https://github.com/apirJS/gemacast/commit/af14292dd73350a1fee12674534d2961bd2c633e))
* PipeWire process-level capture for Linux ([e6460f6](https://github.com/apirJS/gemacast/commit/e6460f6489efbab129d4bca6bd3030f227ab3ed4))
* Windows process-level capture, resampler, manual IP input, toast notifications ([157cd8c](https://github.com/apirJS/gemacast/commit/157cd8c35027bbf188bb7afd7df3cd0a9a81c543))


### Bug Fixes

* ci-cd ([#60](https://github.com/apirJS/gemacast/issues/60)) ([79a7393](https://github.com/apirJS/gemacast/commit/79a73938123621b0f9515bfbceee42fa7c582077))
* harden updater, move APK install to Kotlin, phased builder pattern ([4b4f473](https://github.com/apirJS/gemacast/commit/4b4f473895f83c8c186a397a89178a1da18b3369))
* link the macOS Swift runtime rpath from gemacast-pc's build script ([e95c65e](https://github.com/apirJS/gemacast/commit/e95c65eb1d096b37e25586179ae660465fc093dd))
* mobile UI polish, compatibility improvements ([ae6d549](https://github.com/apirJS/gemacast/commit/ae6d5495fe3464b098b892b7e175bf535c9d9905))
* PipeWire proxy teardown and thread safety ([4372898](https://github.com/apirJS/gemacast/commit/4372898f149a780ce7ee4978698d1d61e754c84e))
* suppress console windows for every subprocess we spawn ([#72](https://github.com/apirJS/gemacast/issues/72)) ([fcb170d](https://github.com/apirJS/gemacast/commit/fcb170d3b841a4432a49b754b18e3e5537579ca4))
* **updater:** updater fixes ([#30](https://github.com/apirJS/gemacast/issues/30)) ([709bdad](https://github.com/apirJS/gemacast/commit/709bdad67a0af9c100c1f0fd6f1be4e84debc834))


### Performance

* jitter manager [skip ci] ([#62](https://github.com/apirJS/gemacast/issues/62)) ([0094df5](https://github.com/apirJS/gemacast/commit/0094df53010da2159c5aa52d4fbfe1d38e4ff23d))
* migrate WASAPI to newer API with cpal fallback ([24a3b4b](https://github.com/apirJS/gemacast/commit/24a3b4b1be453a646a92347fd827c3d938556297))


### Refactoring

* gemacast core with hexagon pattern ([#8](https://github.com/apirJS/gemacast/issues/8)) ([c645ab1](https://github.com/apirJS/gemacast/commit/c645ab15e6d40dd277698b2bb9e8a8ae59bf217b))
* rewrite PC and Mobile with adapter pattern, Mobile migrated to ReactJS ([98e5176](https://github.com/apirJS/gemacast/commit/98e51765528030fba692bdc0155e3eca02a17d76))
* split core into Discovery/Control/Stream modules ([7d5d3cf](https://github.com/apirJS/gemacast/commit/7d5d3cf0b276f9f0c1db4e2e0045616d61e30876))
* UI&UX [skip ci] ([#63](https://github.com/apirJS/gemacast/issues/63)) ([05feb56](https://github.com/apirJS/gemacast/commit/05feb56aa7fe7864abba26bd7571adfcb107c0cc))
