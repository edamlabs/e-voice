# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

Same services, same APIs: the project moves to the EDamLabs organisation.

### Added

- STT diarization node (TODO_vehicle step 2): `[stt.pipeline.diarize]`, off by default, on sherpa-onnx's offline
  speaker diarization (pyannote segmentation 3.0 + WeSpeaker ResNet34-LM), run over an uploaded file beside its
  transcription, and only when the request asks. Each segment takes the speaker that overlaps it longest, numbered
  from 0 in order of first appearance. Live audio carries no speakers.
  - OpenAI: `response_format=diarized_json` (segments with `speaker` `A`, `B`, …; 400 while the node is off; a
    streamed upload emits `transcript.text.segment` events before it ends). Deepgram: `diarize=true` or
    `diarize_model` → `utterances[].speaker`; `words` stays empty, there are no word timestamps. Scribe:
    `diarize=true` → `words[].speaker_id` (`speaker_0`, …). Without the ask every response is unchanged.
  - A diarization that fails after the ask answers 500 in each surface's error envelope, never a 200 without speakers.
  - Evidence: DER 0.3297 on the 8 VoxConverse dev files the radar measured at 0.3298 (`pyannote.metrics` 4.2.0,
    collar 0, overlap scored); a 61-min meeting labelled with 12 voices in 548–617 s on a loaded machine, 442 of 443
    segments with a speaker; 14 response bodies of a 500-segment fixture byte-identical to the previous release.
  - No wall-clock limit fails a segment of an uploaded file: under load a diarized 61-min upload lost 10 of 443
    segments to the ASR deadline (`Timeout`). Its ASR and SER deadlines and its stall limit are now absent, as its
    overload policy; live audio keeps them all. Evidence: `test_file_transcribe_waits_past_the_asr_deadline` red
    (12 of 12) then green.
- Diarization choices for the embedding bench (TODO_vehicle step 3): `embedding` (`wespeaker-resnet34-lm` default,
  `campplus-zh-en`, `eres2net-voxceleb`) and `precision` (`fp32` default, `int8`). No default changes until the
  bench decides.

### Changed

- Home: <https://github.com/edamlabs/e-voice>, documentation <https://edamlabs.github.io/e-voice/>; the old
  `damvolkov/e-voice` URLs redirect.
- Images move with the owner and drop the dash, from the next release: `ghcr.io/edamlabs/evoice-stt`,
  `ghcr.io/edamlabs/evoice-tts` and `ghcr.io/edamlabs/evoice-full` (were `ghcr.io/damvolkov/e-voice-*`).
- Identity declared in `Cargo.toml` (`[workspace.metadata.edamlabs]`: service, eager).
- README: badge layout, landscape logo and description standardized on the EDamLabs template.
- Dependencies: Renovate replaces Dependabot. Routine updates (images, toolchain, actions, lock files, the benchmark's
  Python tooling, Rust patch releases) merge themselves once every required check passes; Rust minor and major
  updates wait for a human.

### Fixed

- STT batch (TODO_vehicle step 1): a file run never sheds a segment. One 1 s chunk carrying several
  utterances met the live overload policy and answered «N segment(s) failed: dropped under load» on long uploads.
  Evidence: `test_file_transcribe_keeps_every_utterance_of_a_dense_second` red before the cut (9 of 12 segments
  `Overload` at `pending = 1`), green after; `test_live_intake_still_drops_a_dense_second` keeps the live drop
  policy; a 61-min meeting through `POST /v1/audio/transcriptions` on CPU: 438 segments, none failed, RTF 0.079,
  peak RSS 4.6 GiB with models loaded.
- Benchmark (`eval/`) pinned to Python 3.14: CI had moved to 3.15.1, where polars 1.44.2 breaks its tests.

### Security

- No number a client sends sizes the services' memory (TODO_VOICE-HELM_harden, in progress). Both listen on
  `0.0.0.0` by default.
  - Sample rate: refused above 384 kHz before any allocation. A rate of `4294967295` — in a socket's `rate`, a
    file's header or a voice clip — reserved about 2.3 GB and aborted the process. Sockets close 1003, uploads 400.
  - Decoded duration: `[stt.api] duration` (default `4h`) bounds what an upload decodes to, as `upload` bounds the
    bytes received; decoding stops at the cap and every door answers 413 in its own error shape. A 7 MB FLAC of ten
    hours of silence costs 10 MiB instead of 2.3 GB. A 61-min meeting still goes through whole (438 segments, RTF
    0.068).
  - Deepgram's prerecorded door answers a body past `upload` in Deepgram's error envelope, not plain text.
  - WebSocket messages are capped: 2 MiB on the audio sockets, 16 MiB on OpenAI Realtime, 64 KiB on the TTS text
    socket (the default let one legal 16 MiB frame cost the native socket 126 MB; a legal 2 MiB frame now costs
    11.6 MiB). A client past the cap is closed with 1009 and a reason, not a normal close; the server does not wait
    for the client's close reply.
  - `POST /v1/voices` stops decoding clips at 30 s in total; its `seconds` now reports the audio learned (at most
    30), not the length uploaded. The audio the voice is learned from is sample-identical to before.
- rustls 0.23.46, rustls-webpki 0.103.16, webpki-roots and webpki-root-certs 1.0.11, tokio 1.54.1: past their
  RustSec advisories. zip stays behind (npyz needs `^0.6`, sherpa-onnx-sys builds with 2.x; the fixes land in
  9.0); e-voice never calls `ZipArchive::extract`, the affected path.
