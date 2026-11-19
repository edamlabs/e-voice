# The voice daemon

`evoiced` is the one process that owns the microphone and the speaker. Everything that wants to speak or
listen on this machine — `ecli`, Claude Code through MCP or hooks, a script — calls it, so two clients never
fight over a device. It calls the two services over their WebSockets and links neither sherpa-onnx nor
onnxruntime.

```bash
evoiced serve                         # 127.0.0.1:5700; --config <path> as the services
```

Start it after `e-voice` and `e-voice-tts` (the units do). It shuts down on SIGTERM. "Sole owner" is a
convention, not a lock: PipeWire lets any process open the same device.

## Security

It listens on loopback and only there: `[daemon] host` is separate from the services' `[server] host`
(`0.0.0.0`), because `/speak` and `/listen` on a network would hand out this machine's speaker and microphone.
There is no authentication and no CORS. Anything wider than `127.0.0.1` is an explicit choice of
`[daemon] host`, with that risk. `/mcp` accepts a loopback `Host` only.

## Routes

| Route | Body | Answer |
|---|---|---|
| `GET /health` | | `{status:"ok", version}` |
| `POST /speak` | `{text, voice?, lang?}` | 200 `{id, outcome: done\|cancelled}` when the audio has drained. 400 empty or over 4096 characters; 429 queue full (16); 502 speech service; 503 output device |
| `POST /stop` | | 200 `{stopped: id\|null, dropped: n}`. Cuts the voice at once, empties the queue, drops a conversation's turn and cancels a recording |
| `POST /listen` | optional `{timeout}` (`"8s"`) | 200 `{text, lang, emotion}`. 400 bad timeout (0 or over 300 s); 408 nobody spoke; 409 the microphone is busy; 502 STT; 503 input device |
| `POST /talk` | | 200 `{state:"listening"}`; 409 if a conversation runs or the microphone is held |
| `DELETE /talk` | | 200 `{state:"idle"}` |
| `POST /voices` | `{id, text?, lang?, silence?, max?, replace?}` | 200 `{id, seconds, text, transcript}`. 400, 408, 409 (busy, exists, cancelled), 502, 503 |
| `POST /voices/finish` | | `{finished: bool}` |
| `GET /voices/script?lang=` | | `{lang, text, sample}`; 400 for a language without script |
| `GET /events` | WebSocket | the event stream below |
| `POST /hooks/stop`, `POST /hooks/notification` | Claude Code hook input | always 200, empty body |
| `/mcp` | MCP over streamable HTTP | the tools below |

Limits: `/speak` and `speak` take at most 4096 characters, a queue of 16 and never more than 2 s of audio held
ahead of the speaker. A `/stop` during a spoken text answers its `/speak` with `cancelled`. `POST /voices`
ends on 2.5 s of silence after speech (`silence`), 30 s (`max`, the most the TTS keeps) or `POST /voices/finish`.

## Events

`GET /events` is a WebSocket of one JSON message per event; `type` is one of `speech` (queue states),
`listening`, `partial` and `heard` (what the microphone is hearing), `talk` (conversation states),
`recording` (`started`, `heard`, `finished`, `learned`, `failed`) and `lagged` (a subscriber that fell behind).
At most 16 subscribers (503 past that) and a ring of 256 events. Close codes: 1003 when the client sends
data, 1008 when it does not read within 5 s, 1009 for a frame over 1 KiB.

## MCP tools

| Tool | Arguments | Result |
|---|---|---|
| `speak` | `text` (≤ 4096), `voice?` | `{id, outcome}` |
| `stop` | | `{stopped, dropped}` — also halts a conversation's turn and cancels a recording |
| `listen` | `timeout_seconds?` (1–300) | `{text, lang, emotion}` |
| `list_voices` | | `{voices: [...]}` |
| `approve` | `tool_name`, `input` | Claude Code's permission prompt for the voice conversation; answers only the conversation's own agent (a per-conversation header) and refuses every other caller |

An error comes back as `{error, status}` with the HTTP status the route would have used. The body of a request
to `/mcp` is capped at 64 KiB. The `X-Evoice-Client` header picks a voice from `[daemon.voices]` for that
client: a preference, never a permission.

## Configuration

`[daemon]`, in the same `evoice.toml` as the services:

| Key | Default | Meaning |
|---|---|---|
| `host`, `port` | `127.0.0.1`, `5700` | where it listens |
| `tts`, `stt` | `ws://127.0.0.1:5600`, `ws://127.0.0.1:5500` | the services' WebSocket bases (`ws://` only) |
| `output`, `input` | unset | a device by part of its name; the system default when unset |
| `listen` | `15s` | how long `/listen` waits for someone to start speaking (at most 300 s) |
| `lang` | `es` | `es` or `en`: the hooks' language when a reply's words do not decide, and the conversation's |
| `voices` | `{}` | `[daemon.voices]`: client name → voice id |
| `talk.idle`, `talk.turn`, `talk.answer` | `60s`, `10m`, `15s` | a conversation ends after this silence; one agent turn is cut past this; a permission question waits this long |
| `talk.claude.bin`, `.cwd`, `.model`, `.tools` | `claude`, the daemon's directory, Claude's default, `["Read","Bash","Edit","Write"]` | the conversation's agent |

Override from the environment as everywhere: `EVOICE_DAEMON__PORT=5701`.

## Claude Code and the conversation

The hooks, the registration, the voice conversation and its permission rules: [Claude Code](claude.md).
The voices recorded through the daemon: [Voices](voices.md).
