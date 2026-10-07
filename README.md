# parakey

Local push-to-talk dictation for Windows, a stand-in for Wispr Flow / FluidVoice.
Hold **Ctrl+Win**, talk, let go. The audio runs through NVIDIA's **Parakeet TDT 0.6B v2**
on your GPU, the text is copied to the clipboard and pasted where your cursor is.
No cloud, no account, no usage limits.

- One ~22 MB exe (Rust), nothing to install. Inference via ONNX Runtime + DirectML, so it runs
  on any Windows GPU without CUDA, and falls back to CPU.
- Idle cost is zero: the mic is closed, no timers run, CPU sits at 0.00%. Only the keyboard
  hook is alive. Pausing removes even that.
- A small recording indicator at the bottom of the screen, a tray icon, and a window with
  pause/resume, your recording history and usage stats.

## Controls

| Keys | What happens |
|---|---|
| Hold **Ctrl+Win** | records while held, pastes when you let go |
| ...then tap **Space** | locks: keeps recording hands-free; press Ctrl+Win again to stop |
| **Esc** while locked | cancels |
| Ctrl+Win + any other key | cancels and passes through, so Ctrl+Win+Left etc still work |
| Tray icon, left-click | opens the window |
| Tray icon, right-click | Open / Pause listening / Open log / Quit |

Taps shorter than 0.3 s are ignored. A forgotten locked recording stops itself after 5 minutes.
Double-clicking a row in the history copies it to the clipboard again.

## Install

Only building needs tools: `winget install Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools`
(with the "Desktop development with C++" workload, which includes the Windows SDK for `rc.exe`).

```powershell
cargo build --release
.\download-model.ps1   # 2.4 GB from Hugging Face -> %LOCALAPPDATA%\parakey\model
.\install.ps1          # copies the exe to %LOCALAPPDATA%\parakey, adds a Start Menu entry, launches
```

After that, press the Windows key and type "parakey" to start it. `.\install.ps1 -Startup` also
launches it at sign-in; `.\install.ps1 -Uninstall` removes the shortcuts. Quit Wispr Flow (or change
its hotkey) first, or both apps will react to Ctrl+Win.

## Files in `%LOCALAPPDATA%\parakey\`

- `model\` — the ONNX model
- `history.tsv` — your recordings: time, audio length, transcription time, word count, text.
  Plain text, so it is readable and greppable; "Clear history" deletes it.
- `parakey.log` — timings and errors only, never transcript text

## Notes

- RAM sits around 400 MB while idle: that is the model staying loaded so transcription is
  instant. Transcribing a 10 s clip takes about 0.3 s on an RTX 4070.
- Apps running as Administrator will not receive the paste unless parakey runs elevated too.
- The window follows the light theme only.
- `assets/nemo128.onnx` (mel-spectrogram preprocessor) comes from
  [onnx-asr](https://github.com/istupakov/onnx-asr), MIT; the model export is
  [istupakov/parakeet-tdt-0.6b-v2-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx), CC-BY-4.0.
