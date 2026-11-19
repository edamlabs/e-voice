# What it is for

One installation, six uses. Everything is local: audio and text never leave the machine, and there is no
account, key or GPU.

| You want | You use | Where |
|---|---|---|
| Claude Code that speaks and listens | the MCP server, the hooks and the voice conversation | `evoiced`, `127.0.0.1:5700` |
| Speech to text inside an application | the OpenAI, Deepgram or ElevenLabs wire protocols, or the native stream | `e-voice`, `:5500` |
| Text to speech inside an application | the OpenAI speech route or the native stream | `e-voice-tts`, `:5600` |
| A voice of your own | `ecli voice add` | the daemon records, the TTS learns |
| To tune what listens | one table per node: wake word, VAD, language, ASR, emotion, speakers | `evoice.toml` |
| A service on the machine | a `.deb` or a tarball with systemd user units, or the Docker images | [Install](install.md) |

## Claude Code, by voice

The daemon owns the microphone and the speaker and serves an [MCP](https://modelcontextprotocol.io) server on
the same queue as its HTTP doors:

```bash
claude mcp add --transport http evoice http://127.0.0.1:5700/mcp
claude mcp add --transport http evoice http://127.0.0.1:5700/mcp --header "X-Evoice-Client: claude"   # its own voice
```

The tools are `speak`, `stop`, `listen` and `list_voices`. A fifth, `approve`, belongs to the voice conversation
and answers only its own agent. Beyond MCP:

- **Hooks.** Claude's reply is spoken when it stops, and a wait for a permission is announced aloud.
- **Dictation.** `ecli listen` returns what you say as text, language and emotion; bind it to a desktop shortcut.
- **Conversation.** `ecli talk` holds a spoken exchange with a headless Claude Code: it listens, answers aloud
  sentence by sentence, asks for a permission by voice and takes a yes or a no. See [Claude Code](claude.md).

Any other MCP client (Cursor, Cline, a script) uses the same `/mcp`. The routes behind it are in the
[daemon reference](daemon.md).

## Speech to text for applications

Base `http://127.0.0.1:5500`. No authentication: any key is accepted and ignored.

| Protocol | Routes | Use with |
|---|---|---|
| OpenAI | `POST /v1/audio/transcriptions` (`json`, `text`, `srt`, `vtt`, `verbose_json`, `diarized_json`; `stream=true` for SSE), `GET /v1/realtime` (Realtime transcription), `GET /v1/models` | the OpenAI SDKs |
| Deepgram | `POST /v1/listen` (a recorded file), `GET /v1/listen` (live, `linear16` or `pcm_f32le`) | clients that speak Deepgram's wire protocol |
| ElevenLabs | `POST /v1/speech-to-text` (Scribe) | clients that speak Scribe's wire protocol |
| Native | `GET /v1/stream` (binary PCM in, JSON events out) | anything with a WebSocket |

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:5500/v1", api_key="unused")
print(client.audio.transcriptions.create(model="whisper-1", file=open("a.mp3", "rb"), language="es").text)
```

The Deepgram and ElevenLabs routes are wire-compatible; they were verified through OpenHuman's client
([Clients](clients.md)), not through those vendors' SDKs. Each service also serves its own OpenAPI document at
`/openapi.json` and an interactive reference at `/docs`. Protocols: [API](../api/index.md).

## Text to speech for applications

Base `http://127.0.0.1:5600`.

```python
client = OpenAI(base_url="http://127.0.0.1:5600/v1", api_key="unused")
client.audio.speech.create(model="pocket", voice="damien", input="Hola.").write_to_file("hola.wav")
```

`POST /v1/audio/speech` answers a file or a stream; `GET /v1/stream` takes text token by token from an LLM and
returns audio while it is generated, with barge-in. Voices: `GET` and `POST /v1/voices`,
`DELETE /v1/voices/{voice_id}`. See [Text to speech](../api/tts.md).

## A voice of your own

```bash
ecli voice add damien2          # read the text on screen; the daemon records and the TTS learns
```

Zero-shot cloning from a clip of 15 to 30 s, in seconds. See [Voices](voices.md).

## What you can tune

The speech-to-text pipeline is one chain of optional nodes, each switched by its own table in `evoice.toml`:

```text
denoise → gain → wake word → VAD → language ID → ASR ∥ emotion → join      (+ speakers, on uploaded files)
```

| Node | Table | Choices |
|---|---|---|
| denoise | `[stt.pipeline.denoise]` | `off`, `gtcrn` |
| wake word | `[stt.pipeline.ww]` | `off`, `kws` (any English phrase), `oww` (`hey_jarvis`); threshold, boost, cooldown |
| VAD and the end of a turn | `[stt.pipeline.vad]` | `silero`, `ten`; `threshold`, `min_silence`, `min_speech`, `max_speech`, `pad` |
| language ID | `[stt.pipeline.lid]` | `off`, `whisper` |
| ASR, live | `[stt.pipeline.asr]` | `nemotron`, `kroko` (streaming); `parakeet`, `canary`, `cohere`, `whisper` (per segment) |
| ASR, uploaded files | `[stt.pipeline.asr.offline]` | any of the above, plus `choices` selectable per request |
| emotion | `[stt.pipeline.ser]` | `emotion2vec`, `emotion2vec-large`, `off` |
| speakers | `[stt.pipeline.diarize]` | `off`, `sherpa`; embedding model, threshold or a fixed speaker count |

There is no separate turn-taking model: a turn ends when the VAD sees `min_silence` of silence (500 ms by
default; it is the latency floor of every final), and a segment is cut at `max_speech`. The voice conversation
adds its own bounds in `[daemon.talk]`: `idle`, `turn` and `answer`. Every key and its default:
[Configuration](configuration.md).

## A service on the machine

An Ubuntu package or tarball installs the binaries and three systemd user units: `evoice-stt`, `evoice-tts` and
`evoiced`. Everything it writes lives under `~/.config/evoice/`. Docker images (`ghcr.io/edamlabs/evoice-stt`,
`-tts`, `-full`) carry the two services; the daemon stays on the host, beside the audio devices. See
[Install](install.md) and [Requirements](requirements.md).
