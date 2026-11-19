# Architecture

Six crates in one workspace, independent but combinable:

| Crate | Is | Depends on |
|---|---|---|
| `core` (`e-voice-core`) | settings loader, model store, onnxruntime binding, audio decode/resample/AGC, logger, process probe, shared contracts (`Lang`, errors) | nothing of ours |
| `stt` (`e-voice-stt`, binary `e-voice`, :5500) | speech-to-text pipeline and its gateway | `core` |
| `tts` (`e-voice-tts`, binary `e-voice-tts`, :5600) | streaming text-to-speech and its gateway | `core` |
| `device` (`e-voice-device`) | microphone capture and speaker playback over cpal (PulseAudio or PipeWire on Linux), the rubato resampler | nothing of ours |
| `daemon` (`e-voice-daemon`, binary `evoiced`, 127.0.0.1:5700) | the process that owns the devices: speech queue, listener, recorder, voice conversation, MCP, hooks | `core` without its `engine` feature, `device` |
| `cli` (`ecli`) | terminal client of the services and the daemon | `device` |

The three binaries read the same `evoice.toml`: `[server]` is shared, each types its own section and keeps
the others' opaque. The two services bind the same `libonnxruntime` (vendored with sherpa-onnx into
`data/ops/sherpa`); the daemon links neither sherpa nor onnxruntime — `core`'s `engine` feature gates
the runtime, the models and audio decoding.

Hexagonal and event-driven. Every node is a port (a trait in `workflow/<node>/base.rs`) with one file per
backend and a registry that maps configuration to a built backend. Backends are built once at startup
and shared read-only by all streams.

## STT

```mermaid
flowchart TB
    subgraph api [api · transports]
        native[native] & openai[openai] & deepgram[deepgram] & elevenlabs[elevenlabs]
    end
    subgraph workflow
        runner[runner] --> session[session · sans-IO state machine]
        runner --> nodes[nodes: denoise · ww · vad · lid · asr · ser]
    end
    api --> live[live bridge / batch] --> runner
    core[core: settings · models · audio · runtime · logger] -.-> workflow
    schema[schema: contracts] -.-> api & workflow & core
```

### Layers

| Layer | Holds | May import |
|---|---|---|
| `schema` | contracts: audio, lang, emotion, segment, event, transcript, error | nothing |
| `config` | one typed section per node and domain | `schema`, `config` |
| `core` | settings, model store, audio decode/resample/AGC, native runtime, logger | `schema`, `config` |
| `workflow` | session, runner, nodes; nodes never import each other | `schema`, `config`, `core` |
| `api` | transports and protocol adapters | everything below |
| `ops` | internal tooling: benchmark suite, wake calibration, voice synthesis | everything below |

`make bounds` enforces the table with grep rules in the gate (the same layering holds in `tts`).

### Session

`Session` is a sans-IO state machine: `SessionInput` in (audio boundaries, partials, results, ticks,
drain, cancel), `SessionOutput` out (commands for the runner, events for the client). It owns the gate,
the join of ASR and SER per segment, deadlines and backpressure. Property tests check:

- exactly one final per segment, in segment order;
- never more than `pending` live segments;
- exactly one `closed`.

### Runner

The runner executes a session against the nodes:

- **Frame path, in order on the stream's task:** denoise → AGC → wake word → VAD → streaming ASR.
- **Segment work (batch ASR, LID, SER):** on the blocking pool, under a semaphore shared by all streams
  (`stt.pipeline.jobs`).
- **Intakes:**
  - `Live` follows the gate and the overload policy.
  - `File` skips the gate and stops reading while the backlog is full, so uploads are lossless.

## TTS

One port, `Synth` (`evoice/tts/src/workflow/synth/base.rs`), and the same shape: backends behind a registry,
a sans-IO session, a runner, transports.

```mermaid
flowchart LR
    text[text deltas · LLM tokens] --> chunker[chunker · UAX #29 sentences, min/max chars]
    chunker --> session[session · sans-IO]
    session -- speak / abort --> runner[runner]
    runner -- one sentence, blocking pool --> synth[Synth backend · frame by frame]
    synth -- 80 ms chunks --> runner --> transport[native WS · OpenAI speech · SSE]
```

- **Streaming only.** `SynthSession::speak` returns a lazy iterator: each `next` computes the next
  chunk, dropping it stops generation. A backend that renders a sentence whole and then slices it is
  rejected by the conformance test every backend must pass (`evoice/tts/tests/workflow/conformance.rs`):
  chunks ≤ 500 ms, a long sentence in more than one chunk, the first chunk before a quarter of the
  total time.
- **Barge-in.** `cancel` aborts the sentence within one chunk (the runner checks between chunks),
  drops what is queued, and keeps the stream open.
- **`train`.** Every backend has it; the default is `Unsupported`. It turns audio clips into an opaque
  voice the same backend opens later. Weight fine-tuning is not part of the port: it is model work
  (Python, GPU) that ends as a new pinned model in `evoice/tts/models.toml`.
- **Workers.** onnxruntime sessions run through `&mut`, so a backend keeps a pool of exclusive workers
  (`tts.synth.workers` per language, `threads` each); a stream leases one for its lifetime, and a full
  pool answers 503 before any audio.
- **Session guarantees** (property-tested): sentences spoken one at a time, in order; one `end` per
  `start`; no audio outside its sentence; one `closed`.

## One onnxruntime

sherpa-onnx and `ort` share one `libonnxruntime` in the process (`ort` binds dynamically to the library
sherpa loaded). Thread spinning is disabled for both, so idle and paced streams cost no CPU. The process holds
the `ort` environment for its whole life (`Runtime::environment`): `ort` tracks it only by a weak reference, so a
dropped environment would destroy it under the sessions.

## Diarization

Speakers are a whole-file node beside the runner, not inside the `Session`: clustering needs the end of the
file. For an upload, `workflow/diarize` (sherpa-onnx's offline pipeline: pyannote segmentation 3.0, an
embedding model, clustering) runs concurrently with the transcription over the same shared samples, and a
two-pointer sweep over the overlap gives each segment its majority speaker. Streaming responses leave `speaker`
empty.

## Daemon

`evoiced` is a hexagon of its own, wired at one root in `main.rs`:

```mermaid
flowchart LR
    ecli[ecli] & mcp[MCP clients] & hooks[Claude Code hooks] --> api[api · HTTP, WebSocket, /mcp]
    api --> queue[speech queue]
    api --> listener[listener]
    api --> recorder[recorder]
    api --> talk[conversation runner]
    queue --> tts[TTS service · ws]
    listener & recorder --> stt[STT service · ws]
    queue --> out[(speaker)]
    listener & recorder --> mic[(microphone lease)]
    talk --> queue & listener
    talk --> agent[headless Claude Code]
    agent -. approve .-> api
```

- **One owner per device.** The microphone is a lease: `/listen`, a recording and a conversation take it in
  turn, and a second taker answers 409. The speaker is one serial queue (16 deep, never more than 2 s of audio
  ahead), so two texts never overlap.
- **One stop.** `/stop` and the MCP `stop` tool call the same sequence: cut the voice at once, halt the
  conversation's turn and wait for its acknowledgement (bounded), cut again. A halt counter fences every push of
  the conversation to the queue, so a halted turn cannot queue anything afterwards.
- **The conversation.** A runner is half-duplex: the microphone is closed while the agent thinks or speaks. The
  agent is a headless Claude Code (`--strict-mcp-config`, `--setting-sources project`, a manual permission mode,
  `--permission-prompt-tool mcp__evoice__approve`, only the tools named in `[daemon.talk.claude] tools`),
  never loading the user's settings. Its replies are cut into sentences and queued as they complete; a
  permission request is read aloud whole and faithfully or refused unasked, and answered with a yes or a no.
- **Ports.** Everything the daemon touches — the speech service, the recognizer, the capture device, the
  playback device, the voice store, the agent — sits behind a trait, so each behaviour is proven with fakes.
