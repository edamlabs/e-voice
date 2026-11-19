# Configuration

One `evoice.toml` for the three processes — `e-voice`, `e-voice-tts` and `evoiced` — read from `--config path`
(a global flag of each; the file must exist) or, without the flag, from `./evoice.toml` when present. The
installed package passes `~/.config/evoice/evoice.toml`. Then `EVOICE_<SECTION>__<KEY>` environment overrides,
e.g. `EVOICE_STT__PIPELINE__ASR__CHUNK=560ms`, `EVOICE_TTS__SYNTH__WORKERS=8` or `EVOICE_DAEMON__PORT=5701`.
Defaults, then the file, then the environment. `[server]` is shared (the daemon uses only its `log`; it binds
`[daemon] host`); each process validates its own section and keeps the others' untouched, so a typo in a
top-level name still fails.

The configuration is a contract:

- every closed choice is an enum — an unknown backend fails to load;
- every table rejects unknown keys;
- values are range-checked (threads ≥ 1, thresholds in range, deadlines positive, backend-specific keys
  only with their backend);
- a configuration that cannot run never starts.

## Sections

| Section | Governs |
|---|---|
| `[server]` | bind address and logging, for every service |
| `[stt]` | default language |
| `[stt.api]` | STT port (5500), upload cap (bytes received), duration cap (what they decode to, `4h`), emotion exposure |
| `[stt.pipeline]` | backpressure, stall timeout, tick, pre-roll, concurrent jobs, gate, gain |
| `[stt.pipeline.denoise]` | speech enhancement before gain and VAD |
| `[stt.pipeline.ww]` | wake word gate |
| `[stt.pipeline.vad]` | segmentation |
| `[stt.pipeline.lid]` | per-segment language identification |
| `[stt.pipeline.asr]` | live and file ASR |
| `[stt.pipeline.ser]` | emotion recognition |
| `[stt.pipeline.diarize]` | speakers of an uploaded file |
| `[stt.ops]` | data root, model manifest, verification depth |
| `[tts]` | default language and voice |
| `[tts.api]` | TTS port (5600), voice-upload cap |
| `[tts.text]` | sentence sizes for synthesis |
| `[tts.synth]` | backend, model size, threads and workers per stream, sampling |
| `[tts.ops]` | data root (models, voices, exports), model manifest, verification depth |
| `[daemon]` | the voice daemon: address, the services it calls, devices, listen time, language |
| `[daemon.voices]` | a voice per MCP client |
| `[daemon.talk]`, `[daemon.talk.claude]` | the voice conversation and its Claude Code agent |

## Every key with its default

The template below is `evoice.example.toml`; a test asserts it equals the built-in defaults.

```toml
--8<-- "evoice.example.toml"
```

## What is checked

Out of range or unusable, a value stops the process at startup:

- **STT.** `stt.api.upload` ≥ 1 MiB; `duration`, `tick` and every deadline > 0; `pending`, `jobs` and every
  `threads` ≥ 1. `gain`: peak and noise ≤ 0 dBFS, max between 0 and 60 dB. `ww.threshold` in (0, 1];
  `ww.boost` only with `kws` and > 0; `ww.keyword` is English letters, spaces and apostrophes with `kws`, `hey_jarvis`
  with `oww`. `vad.threshold` in (0, 1); `min_silence` and `min_speech` > 0; `max_speech` > `min_speech`.
  `asr.chunk` only with `nemotron`; `lead` and `tail` only with a streaming backend.
- **TTS.** `upload` ≥ 1; `text.max` ≥ 8 and `min` < `max`; `threads`, `workers` and `steps` ≥ 1; `temperature` in
  [0, 2]; `voice` is 1–64 characters of `a-z`, `0-9`, `-` or `_`; `api.port` > 0.
- **Daemon.** `port` ≥ 1; `tts` and `stt` start with `ws://`; `listen` in (0, 300 s].

## Notes per node

`asr.backend`
:   Streaming backends (`nemotron`, `kroko`) emit partials while speech lasts; batch backends
    (`parakeet`, `canary`, `cohere`, `whisper`) transcribe each segment when it ends.
    `asr.offline.backend` picks the engine for uploaded files; `live` reuses the live backend.
    `asr.offline.choices` loads further engines that a request selects by its model field (see
    [choosing the engine](../api/index.md#choosing-the-engine)).

`lid`
:   Whisper tiny identifies each segment's language before batch decoding. Canary and Cohere then decode
    in that language, and the final reports it. A detection outside es/en, or a segment shorter than
    `min`, keeps the requested language. Streaming backends fix their language at speech onset.

`denoise`
:   GTCRN runs on every frame before gain and VAD; it delays audio by one model frame.

`vad` and the end of a turn
:   There is no separate turn-taking model. A segment ends after `min_silence` of silence (500 ms: the latency
    floor of every final — lower it for snappier dictation, raise it for slow speakers), is dropped if shorter
    than `min_speech`, and is cut at `max_speech`; `pad` keeps audio around it for the later nodes. `silero` is
    the default, `ten` the alternative; `threshold` is the speech probability.

`ww`
:   The wake word gates the stream: nothing reaches the VAD until the phrase is heard, and `gate.close`
    decides when it shuts again (`utterance`, `window` after `gate.idle`, or `session`). `kws` takes any English
    phrase of two or more words — `make wake WORD="hey eager"` measures the threshold for it; `oww` ships
    `hey_jarvis`. `cooldown` merges close repeats into one wake.

`ser.backend`
:   `emotion2vec` (base) or `emotion2vec-large` (needs the local export, `make export`); `off` removes the
    cost. The per-request `emotion=off` only hides the field; `stt.api.emotion` chooses `field`, `tag`
    (prefixes the text, `[angry] …`) or `off`. Segments shorter than `ser.min` are not classified.

`diarize`
:   Speakers of an uploaded file, once over the whole file; never on live audio. `backend = "sherpa"` runs
    pyannote segmentation 3.0, an embedding model and clustering. `embedding` is `wespeaker-resnet34-lm`
    (default), `campplus-zh-en` or `eres2net-voxceleb`; `precision` `fp32` or `int8`; `threads` is its own budget
    apart from `jobs`; `threshold` (0.5) is the clustering distance and is ignored when `speakers` fixes the
    count; `min_on` and `min_off` shape the turns. While it is `off`, `response_format=diarized_json` answers 400
    and the `diarize` flags of the Deepgram and ElevenLabs routes are ignored. See
    [the diarization bench](../history/2026-11-diarization.md).

`daemon`
:   The voice daemon's keys are in its [own page](daemon.md#configuration). `lang` there is the language of its
    spoken notices and of the conversation; `voices` maps an MCP client to a voice.

`tts.synth.workers`
:   Concurrent streams per language; each holds its own onnxruntime sessions with `threads` threads.
    Pocket base int8 runs ≈ 3× real time per stream on 2 threads, so `workers × threads` should stay
    under the physical cores. A stream beyond `workers` gets 503 before any audio.

`tts.text.max`
:   Pocket tends to end a long sentence early at a clause boundary (upstream torch does the same);
    sentences longer than `max` chars are cut at `, ; :` first, which keeps every clause. 80 is measured.

`tts.synth.backend`
:   `pocket` (default: fastest, ≈ 3× real time per stream), `qwen3` (best intelligibility and
    similarity, ≈ real time per stream: one or two workers), `neutts` (needs `make neutts`,
    `espeak-ng` and voices learned with their transcript). `temperature` unset uses each backend's
    upstream default.

`tts.voice`
:   No backend speaks without a voice prompt. Learn one (`e-voice-tts voice add <id> <files>`) and
    set it here; it is primed at startup.
