# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

Same services, same APIs: the project moves to the EDamLabs organisation.

### Added

- Native Ubuntu packages (TODO_VOICE-HELM_native): a release that includes `full` attaches a `.deb` and a tarball
  for Linux x86_64 to its `full-vX.Y.Z` GitHub release, built on Ubuntu 22.04 (glibc 2.34 floor) behind
  `vars.PUBLISH_PACKAGE`. They carry `e-voice`, `e-voice-tts`, `evoiced` and `ecli`, the sherpa-onnx and ONNX Runtime
  libraries, the manifests and three systemd user units (`evoice-stt`, `evoice-tts`, `evoiced`). Everything the
  installation writes lives under `~/.config/evoice/` — config, models, voices; the default config selects Qwen3
  (public weights) and is copied on first start, never overwritten. `make package` builds both, reproducibly;
  `ecli --version` now reports the release. Evidence: installed, run and removed in clean Ubuntu 22.04 and 24.04
  containers (`systemd-analyze --user verify`, `/health`, `pull` resolving `qwen3-tts`); two builds, one sha256;
  workspace 843 passed; `make coverage` 92.39 %.
- Documentation brought up to date against the code: the voice daemon, what it is for, requirements (ports, paths,
  disk, memory, cores), every configuration table with what is checked, `ecli`'s eight commands, the daemon and
  diarization in the architecture, model sizes, the API pages' limits and `model` field; `openapi-tts.json`
  regenerated (it lacked 413 and 422). Four decision records.
- `ecli voice add <id>` (TODO_VOICE-HELM_clone): a voice learned from a text read aloud. The daemon records from its
  microphone (`POST /voices`; busy while a conversation or a listen holds it) until 2.5 s of silence after speech,
  30 s — the most the TTS keeps — or `POST /voices/finish` (Enter in `ecli`); the clip is cut to the STT's speech
  bounds, sent at 24 kHz mono to the TTS `POST /v1/voices` with its text (yours, else the reading script of
  `GET /voices/script`, else the transcript), and a sample is said in the new voice. An existing id needs
  `--replace`; `/stop` and the MCP `stop` cancel it and nothing is learned. `recording` states on `/events`.
  Evidence: 40 recording proofs (every end condition to the sample, busy, cancel by each door, the uploaded WAV's
  header and sizes, text precedence, 400/408/409/502), `ecli voice` against a fake daemon; workspace 835 passed;
  `make coverage` 92.38 %. Not heard yet on a real microphone.
- `evoiced`, the daemon that owns the microphone and the speaker (TODO_vehicle step 7): `evoiced serve` on
  `127.0.0.1:5700` — loopback unless `[daemon] host` says otherwise, never the services' `[server] host`.
  - `POST /speak {text, voice?, lang?}`: through the TTS service, one voice at a time, answering when the audio has
    drained. At most 4096 characters, a queue of 16 (429 past it), never more than 2 s of audio held ahead of the
    speaker.
  - `POST /stop`: the voice cut within a frame and the queue emptied; the cut requests answer `cancelled`.
  - `POST /listen {timeout?}`: the microphone opened for one utterance, answered as text, language and emotion.
    The timeout (`[daemon] listen`, 15 s; at most 300 s) bounds the wait for someone to start speaking.
  - `GET /events`: a WebSocket of the daemon's events — speech states, listening open and closed, partials, what
    was heard, and `lagged` for a subscriber that fell behind; at most 16 subscribers.
  - `GET /health`. `[daemon]` keys: `host`, `port`, `tts`, `stt`, `output`, `input`, `listen`; the services accept
    the table in the shared `evoice.toml`. A daemon that cannot install its SIGTERM handler does not start.
  - `ecli say | stop | listen`: thin clients of the daemon (`--url`, `ECLI_DAEMON_URL`); `listen --struct` prints
    text, language and emotion as one JSON line.
  - MCP at `/mcp` (TODO_vehicle step 8): the tools `speak`, `stop`, `listen` and `list_voices` on the same queue
    and listener as the HTTP doors, so ids, limits and `/events` are shared. Stateless, served to clients of every
    protocol version; a body of at most 64 KiB; a loopback Host only. `[daemon.voices]` gives a registration its
    own voice, keyed by the `X-Evoice-Client` header — a preference, never a permission.
    `claude mcp add --transport http evoice http://127.0.0.1:5700/mcp --header "X-Evoice-Client: <name>"`.
    The MCP SDK takes the stripped binary from 6.8 to 11.3 MB.
  - Claude Code hooks (TODO_vehicle step 9): `POST /hooks/stop` speaks the first two sentences of Claude's reply —
    prose only, at most 300 characters, in the language of its words; `POST /hooks/notification` says when Claude
    waits for a permission or has been idle. Each answers at once and never slows Claude Code. `[daemon] lang`
    (`es` by default) is the notices' language and the fallback. Guide: `docs/guide/claude.md`, with the
    `settings.json` snippet and dictation by a desktop shortcut over `ecli listen` (press-to-start; hold-to-talk
    is not built).
  - The voice conversation (TODO_vehicle step 10): `POST /talk` and `DELETE /talk`, `ecli talk`. Half-duplex with
    a headless Claude Code: it listens, the agent answers aloud sentence by sentence (fenced code never said), and
    listens again; «adiós» / «goodbye» alone ends it, and so do `[daemon.talk] idle` (60 s) and `turn` (10 min).
    The session is always isolated from the user's Claude Code settings and has only the tools named in
    `[daemon.talk.claude] tools`. Permissions are asked aloud through the MCP tool `approve`, which answers only
    that conversation's agent: only what is read aloud whole and faithfully can be approved — every symbol named,
    over 160 characters or several lines refused unasked; silence or an unclear answer is a no. The microphone is
    the conversation's while it runs. One stop for every door: `/stop` and the MCP `stop` tool cut the voice at
    once, drop the agent's turn and listen again; a halted turn can queue nothing after. The talk states on
    `/events`. Guide: `docs/guide/claude.md`.
  - Evidence: every endpoint proven against scripted TTS and STT servers and fake devices, the MCP tools with a
    real client; the spoken digest held to its properties over 4000 generated inputs — every character a letter, a
    digit, a space or speakable punctuation; `ldd evoiced` free of sherpa-onnx and onnxruntime; `make coverage`
    91.53 % lines. Not proven: no real speaker or microphone has been heard through it, and no Claude Code
    session has called the tools or fired the hooks.
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
  updates wait for a human. `tungstenite` in the services and the daemon never moves alone: it is pinned to the
  one axum resolves.

### Fixed

- `[stt.pipeline.diarize]` is range-checked like every other node: `threads` ≥ 1, `threshold` finite and positive,
  `speakers` ≥ 1 when set (a zero used to reach the node). The «four tools» note of the MCP server says five.
- Renovate #28 broke `main`: `tungstenite` moved to 0.30 in stt and tts, away from axum's 0.29, so an oversized
  WebSocket message closed 1000 instead of 1009; the pin is back. `ort` 2.0.0-rc.15 holds the ONNX Runtime
  environment weakly: it is now kept for the life of the process and handed to every session (NeuTTS, Pocket,
  Qwen3, emotion2vec, openWakeWord). Evidence: the 1009 frame tests green again (stt 11, tts 4, the daemon's
  `/events`); 54 of 54 model-backed tests with real models; the three runtime images build.

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
