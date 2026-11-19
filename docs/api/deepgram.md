# Deepgram

| Parameter | Values |
|---|---|
| `model` | an engine name or model id selects a loaded engine on prerecorded requests ([choosing the engine](index.md#choosing-the-engine)); any other value uses the default; live sockets use the live backend |
| `language` | `es`, `en` (regions accepted) |
| `encoding` | live: `linear16` (default), `pcm_f32le` |
| `sample_rate` | live: rate of the raw audio, 16000 by default |
| `interim_results` | live: `is_final: false` results from partials (default true) |
| `emotion` | `field`, `tag`, `off` (extension) |

## Prerecorded

`POST /v1/listen?language=es` with the audio file as the raw body; the container is sniffed, and
`Content-Type` is a hint.

The response follows Deepgram's shape:

- `metadata` with `request_id`, `duration`, `channels`, `models`;
- `results.channels[0].alternatives[0].transcript` with `detected_language`;
- one `results.utterances[]` entry per segment, with `start`/`end` in seconds and `emotion`.

`diarize=true` or any `diarize_model` adds `speaker` (an integer from 0, in order of first appearance) to each
`results.utterances[]` entry, when `[stt.pipeline.diarize]` is on; while it is off the parameter is ignored. `words`
stays empty: word-level timing is not produced. Live audio carries no speakers.

Errors use `{"err_code", "err_msg", "request_id"}`. A body over `[stt.api] upload` or audio past
`[stt.api] duration` is 413 in that shape.

## Live

`GET /v1/listen?encoding=linear16&sample_rate=48000&language=es` upgrades to a WebSocket.

**Client → server.** Binary audio, messages capped at 2 MiB (past the cap the socket is closed with 1009; the server does not wait for the client's close reply, and a client still sending a single frame larger than the cap may see the connection reset before it reads the close). `{"type":"CloseStream"}` drains pending segments and closes; other
control messages (`KeepAlive`, `Finalize`) are accepted.

**Server → client:**

- `SpeechStarted` when a segment opens;
- `Results` with `is_final: false` from partials;
- `Results` with `is_final: true` and `speech_final: true` per segment;
- `UtteranceEnd` after each final;
- `Metadata` before the close frame.
