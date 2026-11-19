# OpenAI

## Transcriptions

`POST /v1/audio/transcriptions`, multipart:

| Field | Values |
|---|---|
| `file` | wav, mp3, m4a, flac, ogg, opus, webm |
| `model` | any (accepted for compatibility) |
| `language` | `es`, `en` |
| `response_format` | `json` (default), `text`, `srt`, `vtt`, `verbose_json`, `diarized_json` |
| `stream` | `true` → server-sent events |
| `emotion` | `field`, `tag`, `off` (extension) |

`prompt`, `temperature`, `timestamp_granularities[]`, `chunking_strategy`, `known_speaker_names[]` and
`known_speaker_references[]` are accepted and ignored.

- **`json`:** `{"text", "usage", "emotion"}`.
- **`verbose_json`:** adds `language`, `duration` and `segments[]` with `start`/`end` in seconds and a
  per-segment `emotion`.
- **`diarized_json`:** `{"task", "duration", "text", "segments[]", "usage"}`; each segment is
  `{"type": "transcript.text.segment", "id", "start", "end", "speaker", "text"}`, speakers `A`, `B`, … in order of
  first appearance (`AA`, `AB`, … past `Z`). A segment no voice overlaps has no `speaker`. Needs
  `[stt.pipeline.diarize]` on: a 400 otherwise. With `stream=true`, the deltas are followed by one
  `transcript.text.segment` per segment before `transcript.text.done`. `verbose_json` never carries speakers.
- **`stream=true`:** one `transcript.text.delta` per final, then `transcript.text.done` with `usage`.
- **Errors:** OpenAI's shape, `{"error": {"message", "type", "param", "code"}}`.
- **Failure policy:** a request whose segments fail is a 500, never silently partial text.

## Realtime

`GET /v1/realtime?intent=transcription&language=es&emotion=field` upgrades to the Realtime transcription
protocol. Both the GA (`session.*`) and beta (`transcription_session.*`) event names are served.

| Client event | Effect |
|---|---|
| `session.update` / `transcription_session.update` | sets language (`audio.input.transcription.language` or `input_audio_transcription.language`) and rate (`audio.input.format.rate`, default 24000) before audio starts |
| `input_audio_buffer.append` | base64 pcm16 audio |
| `input_audio_buffer.commit` / `clear` | acknowledged; server VAD always segments |
| `session.close` | drains pending segments, then closes with 1000 |

Events are capped at 16 MiB: past the cap the socket is closed with 1009; the server does not wait for the client's close reply, and a client still sending a single frame larger than the cap may see the connection reset before it reads the close.

Server events:

- `session.created` / `session.updated`;
- `input_audio_buffer.speech_started` / `speech_stopped` / `committed`;
- `conversation.item.input_audio_transcription.delta` (from streaming partials);
- `…completed` (with the `emotion` extension) and `…failed`;
- `error`.
