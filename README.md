# What Was That About

A desktop app for Windows, Linux and macOS that transcribes **Persian (Farsi)** audio and video
**on your own computer**, then helps you find out what was said: read along while it plays,
click any sentence to hear it, search across all your recordings, and (optionally, with an
OpenRouter key) get summaries, chapters, tags, answers with clickable citations, and small
interactive HTML docs generated from the transcript.

Everything except the optional AI features works **offline**. Speech models are not bundled;
the app downloads them on first use, or you can import files you downloaded yourself.

## Speech models

| Model | Best for | License | Download | Notes |
|---|---|---|---|---|
| [Taraneh-WLarge](https://huggingface.co/areffarhadi/Taraneh-WLarge) | Best accuracy with a GPU | MIT | 6.2 GB → 1.7 GB on disk | Whisper large-v3 fine-tuned on ~1,560 h of Persian (3.9% WER on FLEURS). Runs on whisper.cpp with **Vulkan** (Windows/Linux, any modern GPU) or **Metal** (macOS). Works on CPU, slowly. |
| [Shenava Koochik v1.0](https://huggingface.co/Reza2kn/Shenava-Koochik-v1.0) | CPU, laptops, long files | Apache-2.0 | 459 MB | 114M FastConformer CTC (10.6% WER on FLEURS). ~30× faster than real time on an 8-core CPU. |

Install them from **Models** in the app. Taraneh is published as PyTorch safetensors; the app
converts it on your machine into a whisper.cpp model (Q8_0 by default, or F16). The converter is
a Rust port of whisper.cpp's `convert-h5-to-ggml.py` + `quantize` and produces byte-identical
files, so no Python is needed.

**Offline / manual install:** every model card lists the exact file URLs. Download them on any
computer, copy them over, and use **Import files…**. For Taraneh you can also import a
ready-made whisper.cpp `ggml-*.bin`. Downloads resume after interruptions and are verified with
SHA-256.

## Features

- Import audio or video by drag and drop (mp3, m4a/aac, wav, flac, ogg/vorbis, opus, webm, mp4,
  mkv…). Formats the native decoder can't read are decoded through the web view as a fallback.
- Local transcription with sentence-level segments and word timings (Silero VAD + DTW alignment
  for Whisper), Persian digits for spoken numbers (Shenava), editable segments.
- Player with waveform, chapters, speed control and karaoke-style highlighting; click a sentence
  to play it. Keyboard: <kbd>Space</kbd> play/pause, <kbd>←</kbd>/<kbd>→</kbd> ±5 s,
  <kbd>Shift</kbd>+<kbd>←</kbd>/<kbd>→</kbd> previous/next sentence, <kbd>Ctrl/⌘ K</kbd> search,
  <kbd>Ctrl/⌘ F</kbd> find in transcript, <kbd>?</kbd> all shortcuts.
- Library search that ignores Arabic/Persian letter variants, diacritics and ZWNJ; tag browser
  across recordings; "related sections" (lexical offline, semantic with embeddings).
- Export as TXT, SRT, VTT, Markdown or JSON.

### AI features (optional, OpenRouter)

Paste an [OpenRouter](https://openrouter.ai/keys) API key in **Settings** and pick a model.
Cheap defaults: `deepseek/deepseek-v4-flash`, `openai/gpt-6-luna`, `anthropic/claude-haiku-5.5`.
A one-hour recording is roughly 40k tokens, a fraction of a cent with these models.

- **Analyze:** title, summary, key points, chapters, topic tags with the segments that discuss
  them, action items and suggested questions.
- **Ask:** chat about one recording or the whole library; answers cite segments like `[12]`,
  and clicking a citation plays that moment.
- **Docs:** the model writes a self-contained HTML/JS page (mind map, timeline, flashcards,
  quiz, minutes…) shown inside the app. Docs run in a sandboxed iframe on their own origin with
  a Content-Security-Policy that blocks all network access; they can only read the transcript
  through a small `window.WWTA` bridge and ask the player to seek.
- **Semantic search:** optional embeddings (default `baai/bge-m3`) for meaning-based search and
  related sections.

Only transcript text is sent to OpenRouter, and only when you use these features. Audio never
leaves your computer.

## Install

Get installers from [Releases](https://github.com/deep-in-apeirophobia/what-was-that-about/releases)
or from the artifacts of the latest [CI run](https://github.com/deep-in-apeirophobia/what-was-that-about/actions).

- **Windows 10/11:** `.exe` (per-user installer) or `.msi`. The installers aren't code-signed
  yet, so SmartScreen may warn ("More info → Run anyway"). GPU transcription uses Vulkan, which
  ships with current NVIDIA/AMD/Intel drivers. Without it the app still runs, and Shenava works.
- **Linux:** `.AppImage`, `.deb` or `.rpm`. Needs `libvulkan1` (`vulkan-loader` on Fedora) for
  Whisper. On NixOS use `appimage-run`, or build from source with `nix develop`.
- **macOS 11+:** `.dmg` (Apple Silicon and Intel). Builds aren't notarized: right-click the app →
  Open the first time, or run `xattr -dr com.apple.quarantine "/Applications/What Was That About.app"`.

## Build from source

Prerequisites: Rust (stable), Node 22 + pnpm, CMake, a C/C++ toolchain, and
- Windows: Visual Studio Build Tools (C++), the [Vulkan SDK](https://vulkan.lunarg.com/sdk/home)
  (`VULKAN_SDK` set), LLVM/Clang for bindgen.
- Linux: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libsoup-3.0-dev libssl-dev clang
  libclang-dev`, plus the Vulkan SDK (or distro `libvulkan-dev` + `glslc`). NixOS: `nix develop`.
- macOS: Xcode command line tools.

```sh
pnpm install
pnpm tauri dev          # run the app
pnpm tauri build        # installers in target/release/bundle
pnpm dev                # UI only, in a browser, against an in-memory mock backend
```

The first build downloads sherpa-onnx's prebuilt libraries and compiles whisper.cpp (several
minutes).

### Command-line tool

`crates/wwta-cli` builds a `wwta` binary that uses the same engine:

```sh
cargo run --release -p wwta-cli -- info
cargo run --release -p wwta-cli -- install shenava-koochik --models-dir ./models
cargo run --release -p wwta-cli -- transcribe talk.mp3 --model-file ./models/shenava-koochik/model.onnx --kind shenava
cargo run --release -p wwta-cli -- convert ./Taraneh-WLarge ./ggml-model-q8_0.bin   # HF safetensors -> whisper.cpp
```

### Tests

```sh
cargo test -p wwta-core   # core library (no native engines needed)
pnpm test                 # UI logic
```

## Architecture

```
crates/wwta-core     audio decode/resample (Symphonia), Silero VAD + engines (sherpa-onnx, whisper.cpp),
                     safetensors→GGML converter, model catalog/downloads, SQLite library, search,
                     OpenRouter client and prompts, exports
crates/wwta-cli      headless CLI (also the CI smoke test)
src-tauri            Tauri v2 app: commands, transcription queue, wwta-doc:// protocol for docs
src                  React 19 + TypeScript UI (Vite, Tailwind, wavesurfer.js, zustand)
```

Some implementation notes:
- Whisper runs on VAD-aligned chunks of up to ~28 s. whisper.cpp's built-in VAD doesn't remap
  token timestamps, so this keeps word timings exact.
- On Linux, whisper.cpp is compiled with `-D_GLIBCXX_USE_CXX11_ABI=0` to match sherpa-onnx's
  prebuilt libraries (see `.cargo/config.toml`). On Windows everything uses the static CRT, and
  `vulkan-1.dll` is delay-loaded so the app starts on machines without GPU drivers.

## Credits

- Taraneh-WLarge by Aref Farhadipour (University of Zurich), MIT, based on OpenAI Whisper (MIT).
- Shenava Koochik by Reza2kn / PersianML, Apache-2.0.
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (MIT), [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
  (Apache-2.0), [Silero VAD](https://github.com/snakers4/silero-vad) (MIT, bundled),
  [Symphonia](https://github.com/pdeljanov/Symphonia) (MPL-2.0), Tauri, React, wavesurfer.js, Vazirmatn font (OFL).
- Test fixture: an excerpt of Radio Geek #2 by Jadi, CC BY-SA 4.0 (see `tests/fixtures/README.md`).

## License

MIT. See [LICENSE](LICENSE). Model weights have their own licenses (above).
