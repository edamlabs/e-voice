# e-voice

CPU-first speech in Rust, all local: speech to text (`e-voice`, :5500), text to speech (`e-voice-tts`, :5600) and
a voice daemon (`evoiced`, 127.0.0.1:5700) that gives Claude Code a voice. Two services on one shared core, a
daemon that owns the microphone and the speaker, and a CLI (`ecli`).

Part of the **EDamLabs eager series**: EDamLabs' own e- products ([EDamLabs families](https://github.com/edamlabs/esteward#identity)).

- **For Claude Code:** an MCP server, hooks that speak its replies, dictation and a spoken conversation —
  [the daemon](guide/daemon.md).
- **For applications:** STT and TTS behind OpenAI, Deepgram and ElevenLabs compatible routes and a native
  WebSocket — [what it is for](guide/uses.md).
- **Your own voice:** `ecli voice add` records it, the TTS learns it in seconds — [voices](guide/voices.md).
- **As a service:** an Ubuntu `.deb` or tarball with systemd user units, or Docker images —
  [install](guide/install.md), [requirements](guide/requirements.md).

## Speech to text

One pipeline — denoise → wake word → VAD → language ID → ASR ∥ SER →
join — behind native, OpenAI, Deepgram and ElevenLabs compatible APIs. Spanish and English, an emotion
on every segment, speakers on uploaded files, no GPU.

```mermaid
flowchart LR
    audio([audio]) --> denoise[denoise] --> gain[AGC] --> ww[wake word] --> vad[VAD]
    vad -- segment --> lid[language ID] --> asr[ASR]
    vad -- segment --> ser[SER]
    vad -. live audio .-> asr
    asr --> join{{join}}
    ser --> join
    join --> final([final: text · lang · emotion])
    file([uploaded file]) -. whole file .-> dia[diarize] -. speaker .-> final
```

Every node is a port with swappable backends chosen in [configuration](guide/configuration.md); the
defaults are what the [benchmark](benchmark.md) measured best on CPU.

| Node | Default | Alternatives |
|---|---|---|
| denoise | off | GTCRN |
| ww | off | sherpa KWS (`hey eager`), openWakeWord |
| vad | Silero | TEN |
| lid | off | Whisper tiny |
| asr (live) | Nemotron 3.5 streaming | Kroko, Parakeet, Canary, Cohere, Whisper |
| asr (files) | Parakeet TDT v3 | Canary, Cohere, Whisper, the live backend |
| ser | emotion2vec+ base | emotion2vec+ large, off |
| diarize | off | sherpa-onnx (pyannote 3.0 + WeSpeaker), uploaded files only |

## Quickstart

On Ubuntu, from the [package](guide/install.md#ubuntu-package):

```bash
sudo apt install ./evoice_<version>_amd64.deb
systemctl --user enable --now evoice-stt evoice-tts evoiced
```

From source:

```bash
make install             # OS packages, pinned Rust, uv, prek, llvm-cov, git hooks
make setup               # sherpa-onnx libraries + the models the config declares
make serve               # gateway on :5500 — reference at http://127.0.0.1:5500/docs
make stt ARGS="--flat"   # another terminal: talk; Ctrl+C to finish (the Makefile points ecli at :45140 and :45150)
```

Or in a container: `make up` (see [Docker](guide/docker.md)). Clients point at `127.0.0.1:5500` either
way.

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:5500/v1", api_key="unused")
print(client.audio.transcriptions.create(model="whisper-1", file=open("a.mp3", "rb"), language="es").text)
```

## Text to speech

Text in — whole, or token by token from an LLM — and audio out while it is generated, in a voice
learned from a few seconds of audio. Three backends, all on our own streaming loop: Kyutai Pocket TTS
(the code default, fastest), Qwen3-TTS (the package's default: public weights, closest to the speaker) and
NeuTTS; the port admits only backends that stream frame by frame ([backends](backends.md#tts)).

```bash
make pocket                                          # export + install the Pocket bundles (gated checkpoint)
ecli voice add damien                                # record a voice through the daemon, or:
make voice ID=damien FILES=me.mp3                    # learn one from files
EVOICE_TTS__VOICE=damien make speak                  # gateway on :5600
make tts ARGS='"Hola, soy Demian."'                  # hear it
```

```python
client = OpenAI(base_url="http://127.0.0.1:5600/v1", api_key="unused")
client.audio.speech.create(model="pocket", voice="damien", input="Hola.").write_to_file("hola.wav")
```

See [Text to speech](api/tts.md) and [Voices](guide/voices.md).
