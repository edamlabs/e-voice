# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

Same services, same APIs: the project moves to the EDamLabs organisation.

### Changed

- Home: <https://github.com/edamlabs/e-voice>, documentation <https://edamlabs.github.io/e-voice/>; the old
  `damvolkov/e-voice` URLs redirect.
- Images move with the owner and drop the dash, from the next release: `ghcr.io/edamlabs/evoice-stt`,
  `ghcr.io/edamlabs/evoice-tts` and `ghcr.io/edamlabs/evoice-full` (were `ghcr.io/damvolkov/e-voice-*`).
- Identity declared in `Cargo.toml` (`[workspace.metadata.edamlabs]`: service, eager).
- Dependencies: Renovate replaces Dependabot. Routine updates (images, toolchain, actions, lock files, the benchmark's
  Python tooling, Rust patch releases) merge themselves once every required check passes; Rust minor and major
  updates wait for a human.

### Security

- rustls 0.23.46, rustls-webpki 0.103.16, webpki-roots and webpki-root-certs 1.0.11, tokio 1.54.1: past their
  RustSec advisories. zip stays behind (npyz needs `^0.6`, sherpa-onnx-sys builds with 2.x; the fixes land in
  9.0); e-voice never calls `ZipArchive::extract`, the affected path.
