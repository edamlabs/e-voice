# Requirements

CPU only. Figures below are measured on this project's reference machine (an i9-14900K, 32 threads) or read
from the installed model folders; where nothing was measured the page says so.

## Ports and paths

| What | Default | Set by |
|---|---|---|
| STT service | `0.0.0.0:5500` | `[stt.api] port`, `[server] host` |
| TTS service | `0.0.0.0:5600` | `[tts.api] port`, `[server] host` |
| Voice daemon | `127.0.0.1:5700` | `[daemon] host`, `port` |
| Configuration | `~/.config/evoice/evoice.toml` (package) · `./evoice.toml` or `--config` (source) | |
| Models and voices | `~/.config/evoice/stt`, `~/.config/evoice/tts` (package) · `data/stt`, `data/tts` (source) | `[stt.ops] data`, `[tts.ops] data` |

The two services bind every interface and have no authentication: put them behind a firewall or set
`[server] host = "127.0.0.1"` when the machine is not yours alone.

## Disk

Models are downloaded by each service's `pull` on first start. Sizes as installed:

| Model | Disk |
|---|---|
| `nemotron-3.5-1120ms-int8` (default live ASR) | 653 MB |
| `parakeet-v3-int8` (default for uploads) | 641 MB |
| `emotion2vec-plus-base` (emotion) | 357 MB |
| `silero-vad` | 0.6 MB |
| **Default STT set** | **≈ 1.65 GB** |
| `qwen3-tts` (the package's TTS) | 3.6 GB |
| `pocket-es`, `pocket-en` | 524 MB each |
| `whisper-turbo` / `whisper-tiny` | 990 MB / 245 MB |
| `canary-180m` | 198 MB |
| `cohere-transcribe` | 2.7 GB |
| diarization: segmentation 7.3 MB + `wespeaker-resnet34-lm` 26 MB | 33 MB |
| sherpa-onnx and onnxruntime libraries | 31 MB |

The package itself is about 29 MB. A learned voice is a few hundred KB.

## Memory

Peak resident memory of one STT service transcribing files with four items in flight (the backends benchmark,
32 cores): Kroko 1.6 GB, Nemotron 2.1 GB, Canary 2.0 GB, Parakeet 2.8 GB, Whisper turbo 4.5 GB, Cohere 5.9 GB;
emotion2vec large adds about 0.4 GB. A decoded upload is held in memory: up to 922 MB at the 4-hour default of
`[stt.api] duration`. The TTS service and the daemon have no memory figure yet — not measured.

## Cores

Measured with 32 threads: the default STT chain costs 0.25 CPU-seconds per audio second with Parakeet
(25× real time) and 0.41 with Nemotron. Pocket synthesises at a real-time factor of 0.33 on two threads
(first audio ≈ 160 ms, six times real time across four streams); Qwen3 at 0.96 (first audio ≈ 0.4 s). A
61-minute meeting transcribes in about 290 s with speakers off.

Default thread budgets: ASR 4, offline ASR 4, emotion 4, diarization 4, language ID 2, VAD 1, wake word 1,
denoise 1; `jobs = 8` segments in flight across streams; TTS 2 threads × 4 workers per language (the package
sets Qwen3 to 2 workers × 4 threads). Keep `workers × threads` under the physical cores: a stream beyond
`workers` gets 503 before any audio.

## Operating system

- **Linux x86_64**, glibc of Ubuntu 22.04 or later, ALSA (`libasound2`) and `espeak-ng`. The audio goes
  through PulseAudio or PipeWire.
- From source, the Makefile has install targets for macOS and Windows as well; the Ubuntu package is the only
  prebuilt distribution.
- `claude` on the PATH for the voice conversation.
