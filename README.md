# Flowe

Local push-to-talk dictation for Windows, a stand-in for Wispr Flow / FluidVoice.
Hold **Ctrl+Win**, talk, let go. The audio runs through NVIDIA's **Parakeet TDT 0.6B v2**
on your GPU, the text is copied to the clipboard and pasted where your cursor is.
No cloud, no account, no usage limits.

- One ~22 MB exe (Rust), nothing to install. Inference via ONNX Runtime + DirectML, so it runs
  on any Windows GPU without CUDA, and falls back to CPU.
- Idle cost is zero: the mic is closed, no timers run, CPU sits at 0.00%. Only the keyboard
  hook is alive. Pausing removes even that.
- A tiny dash just above the taskbar means it is listening. While recording it becomes a pill with
  a state indicator (red dot = recording, amber padlock = locked) and a visualization you pick in
  the dashboard: **Waves** (level bars), **Liquid** (spring-based water sim with splashes and
  droplets), **Plasma** (colour field that speeds up and gains contrast as you talk) or
  **Lava lamp** (merging glowing blobs). Picking one plays a short preview.
- Soft "thock" sounds (like a creamy mechanical keyboard) play when recording starts and stops,
  and a double-tap confirms Space-lock
  (switch in the dashboard). Plasma builds a random colour palette every recording.
- An "F" badge sits in the tray, and the dashboard window (opens on launch, closing it
  only hides it) has pause/resume, your recording history and usage stats.

## Controls

| Keys | What happens |
|---|---|
| Hold **Ctrl+Win** | records while held, pastes when you let go |
| ...then tap **Space** | locks: keeps recording hands-free; press Ctrl+Win again to stop |
| **Esc** while locked | cancels |
| Ctrl+Win + any other key | cancels and passes through, so Ctrl+Win+Left etc still work |
| Tray icon, left-click | opens the dashboard (so does launching Flowe again) |
| Tray icon, right-click | Open / Pause listening / Open log / Quit |

Taps shorter than 0.3 s are ignored. A forgotten locked recording stops itself after 5 minutes.
Double-clicking a row in the history copies it to the clipboard again.

## Install

Only building needs tools: `winget install Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools`
(with the "Desktop development with C++" workload, which includes the Windows SDK for `rc.exe`).

```powershell
cargo build --release
.\download-model.ps1   # 2.4 GB from Hugging Face -> %LOCALAPPDATA%\Flowe\model
.\install.ps1          # copies the exe to %LOCALAPPDATA%\Flowe, adds a Start Menu entry, launches
```

After that, press the Windows key and type "Flowe" to start it. `.\install.ps1 -Startup` also
launches it at sign-in, hidden in the tray (`flowe.exe --tray`); `.\install.ps1 -Uninstall`
removes the shortcuts. `cargo run --release --bin mkicon` regenerates the app icon. Quit Wispr Flow (or change
its hotkey) first, or both apps will react to Ctrl+Win.

## Files in `%LOCALAPPDATA%\Flowe\`

- `model\` — the ONNX model
- `history.tsv` — your recordings: time, audio length, transcription time, word count, text.
  Plain text, so it is readable and greppable; "Clear history" deletes it.
- `flowe.log` — timings and errors only, never transcript text
- `settings.txt` — indicator style and sounds on/off

## Notes

- RAM sits around 400 MB while idle: that is the model staying loaded so transcription is
  instant. Transcribing a 10 s clip takes about 0.3 s on an RTX 4070.
- Apps running as Administrator will not receive the paste unless Flowe runs elevated too.
- The dashboard is dark-only (custom drawn), with a dark title bar on Windows 11.
- `assets/nemo128.onnx` (mel-spectrogram preprocessor) comes from
  [onnx-asr](https://github.com/istupakov/onnx-asr), MIT; the model export is
  [istupakov/parakeet-tdt-0.6b-v2-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx), CC-BY-4.0.
