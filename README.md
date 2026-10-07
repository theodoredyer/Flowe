# parakey

Barebones local dictation for Windows, a stand-in for Wispr Flow / FluidVoice.
Hold a key, talk, let go. The audio runs through NVIDIA's **Parakeet TDT 0.6B v2**
locally and the text gets pasted into whatever window has focus. No cloud and no usage limits.

- Inference: [onnx-asr](https://github.com/istupakov/onnx-asr) + ONNX Runtime **DirectML**, which runs on any Windows GPU with no CUDA install (falls back to CPU)
- Speed: ~0.1-0.3s per utterance on an RTX 4070 (~1.5s on CPU)

## Run

Needs [uv](https://docs.astral.sh/uv/) (`winget install astral-sh.uv`).

```
parakey.bat                      # hold Right Ctrl to talk
parakey.bat --key f9             # different hold key
parakey.bat --toggle --key "ctrl+alt+space"   # press to start, press again to stop
parakey.bat --model nemo-parakeet-tdt-0.6b-v3 # multilingual
```

The first run downloads the model (~2.4GB) into `~/.cache/huggingface`.
You'll hear a high beep when recording starts and a low beep when it stops.

## Notes

- The mic stream stays open while parakey runs so the first word never gets clipped,
  so Windows will show the mic-in-use icon.
- Your clipboard is restored after pasting (text only).
- Pasting into apps running as Administrator requires running parakey as Administrator too.
- Start on login: put a shortcut to `parakey.bat` in `shell:startup`.
