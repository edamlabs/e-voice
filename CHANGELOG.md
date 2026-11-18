# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

Same services, same APIs: the project moves to the EDamLabs organisation.

### Added

- `evoiced`, the daemon that owns the speaker (TODO_vehicle step 7, in progress): `evoiced serve` on
  `127.0.0.1:5700` — loopback unless `[daemon] host` says otherwise, never the services' `[server] host`.
  `GET /health`; `POST /speak {text, voice?, lang?}` plays through the TTS service one voice at a time and answers
  when the audio has drained (a queue of 16, 429 past it). `[daemon]` keys: `port`, `host`, `tts`, `output`. The
  services accept the `[daemon]` table of the shared `evoice.toml`. Evidence: 76 daemon tests with a scripted TTS
  server and a fake sink; `ldd evoiced` free of sherpa-onnx and onnxruntime; `make coverage` 90.32 % lines.
  Not yet: `/stop`, `/listen`, `/events`; no real speaker has been heard through it.
- `evoice/device`: the microphone and speaker code of `ecli` as a library; `ecli`'s commands and output are
  byte-identical. `evoice/core` gains an `engine` feature (on by default) so a crate can use its settings, logging
  and resampling without linking the model runtime.
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
- Diarization choices (TODO_vehicle step 3): `embedding` (`wespeaker-resnet34-lm` default, `campplus-zh-en`,
  `eres2net-voxceleb`) and `precision` (`fp32` default, `int8`). Bench on 12 held-out VoxConverse files, each arm
  at its tuned threshold: DER 0.373 for the default, 0.179 for CAM++, 0.141 for ERes2Net (both better by more than
  the registered 0.02, paired intervals above 0.10), int8 no different. The default does not change, and no
  setting is validated for Spanish: on a real 61-min meeting of 9 to 13 speakers the tuned thresholds find 35 to
  49 and the untuned default 24, one of them holding 52 % of the speech. Report:
  `docs/history/2026-11-diarization.md`.
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

- No number a client sends sizes the services' memory (TODO_VOICE-HELM_harden). Both listen on
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
  - TTS text: at most 4096 characters per request on `/v1/audio/speech` and per `text` message on the native
    socket; that door's body is capped at 64 KiB. A malformed, invalid or oversized body is now answered in the
    JSON error shape with its own status (400, 413, 415, 422) — it used to be plain text.
  - Header numbers (channels, packet sizes) were read in the decoder and size nothing beyond one packet: a
    26-channel WAV decodes, 27 channels are refused. ADPCM is not compiled in.
  - `POST /v1/voices` stops decoding clips at 30 s in total; its `seconds` now reports the audio learned (at most
    30), not the length uploaded. The audio the voice is learned from is sample-identical to before.
- rustls 0.23.46, rustls-webpki 0.103.16, webpki-roots and webpki-root-certs 1.0.11, tokio 1.54.1: past their
  RustSec advisories. zip stays behind (npyz needs `^0.6`, sherpa-onnx-sys builds with 2.x; the fixes land in
  9.0); e-voice never calls `ZipArchive::extract`, the affected path.
