# Install

## Ubuntu package

Every release publishes, on its `full-v<version>` GitHub release, a `.deb` and a tarball for Linux x86_64 with
the four binaries (`e-voice`, `e-voice-tts`, `evoiced`, `ecli`), the sherpa-onnx and ONNX Runtime libraries they
load (CPU), the model manifests and three systemd user units. Built on Ubuntu 22.04: it runs on 22.04 and later.

```bash
gh release download full-v<version> -R edamlabs/e-voice -p 'evoice_*_amd64.deb'
sudo apt install ./evoice_<version>_amd64.deb
systemctl --user enable --now evoice-stt evoice-tts evoiced
```

The units run as you, not as root — the daemon needs your audio session. Everything the package writes lives
under `~/.config/evoice/`:

| What | Where |
|---|---|
| Configuration | `~/.config/evoice/evoice.toml` — copied from `/usr/share/evoice/evoice.toml` on first start and never overwritten; every key is in `/usr/share/doc/evoice/evoice.example.toml` |
| Speech-to-text models | `~/.config/evoice/stt` |
| Text-to-speech models and voices | `~/.config/evoice/tts` |
| Ports | STT `5500`, TTS `5600`, daemon `127.0.0.1:5700` |
| Logs | `journalctl --user -u evoice-stt -u evoice-tts -u evoiced` |

Each service's `pull` downloads its models before it serves, so the first start takes as long as the download:
about 1.65 GB for the default speech-to-text set and 3.6 GB for the package's text-to-speech model, Qwen3 (the
package's default config selects it because its weights are public; the code default, Pocket, needs local exports
of a gated checkpoint). [Requirements](requirements.md) has every size.

**Qwen3 speaks only in a voice you have learned**, so before the first synthesis, once `evoiced` is up:

```bash
ecli voice add me                  # read the text on screen; Enter starts and finishes
```

then set it as the default in `~/.config/evoice/evoice.toml` and restart the TTS:

```toml
[tts]
voice = "me"
```

```bash
systemctl --user restart evoice-tts
ecli say "Hola, esta es mi voz."
```

Without apt, the tarball holds the same files and a five-line README: unpack under `~/.local/opt`, put its `bin/`
on the PATH, copy its units to `~/.config/systemd/user/`. To remove the package: `sudo apt remove evoice`; your
`~/.config/evoice/` stays until you delete it.

What the installation is for, and what each part serves: [What it is for](uses.md).

## From source

```bash
make install
```

`install` runs three targets, each usable alone:

| Target | Does |
|---|---|
| `system` | OS build packages: C/C++ compiler, cmake, pkg-config, curl, bzip2, and ALSA headers on Linux |
| `toolchain` | rustup if missing, the toolchain pinned in `rust-toolchain.toml` (1.94.0 with clippy, rustfmt, llvm-tools), uv if missing |
| `tools` | prek and cargo-llvm-cov, then the git hooks |

The platform is detected; force it with `OS`:

=== "Ubuntu / Debian"

    ```bash
    make install OS=ubuntu      # apt; sudo only when not root
    ```

=== "macOS"

    ```bash
    make install OS=mac         # Homebrew (bash, cmake, pkg-config) + Xcode command line tools
    ```

=== "Windows"

    ```bash
    make install OS=windows     # from Git Bash: winget MSVC build tools + CMake
    ```

Then:

```bash
make setup                  # sherpa-onnx shared libraries + the configured models
make setup ARGS=--all       # every model in the manifest (tests, benchmark, wake calibration)
```

`make sherpa` vendors the pinned sherpa-onnx 1.13.8 shared libraries (with onnxruntime 1.28.2) for
linux-x64, macOS universal2 or windows-x64 into `data/ops/sherpa`; binaries find them through an
rpath (`$ORIGIN` / `@loader_path`) or, on Windows, a copy beside the executable.

!!! note "Platforms"
    CI builds and tests Linux x86-64. macOS and Windows are wired (packages, libraries, linker paths,
    audio host) but not exercised by CI.

## Everything lives in `data/`

The repository ignores `data/` except its `.gitkeep`:

```
data/
├── ops/
│   ├── target         cargo build output
│   ├── sherpa         sherpa-onnx + onnxruntime libraries, shared by every service
│   ├── hf             Hugging Face token and transfer cache (never ~/.cache)
│   └── site           built documentation
├── stt/
│   ├── models         pipeline models (the Docker volume)
│   └── ops/           tool and test models, exports, datasets, results
└── tts/
    ├── models         synthesis models (the Docker volume)
    ├── voices         learned voices
    └── ops/           checkpoints, exporter, exports (+ goldens), tool models, results
```
