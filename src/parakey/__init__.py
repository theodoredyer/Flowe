"""parakey: hold a hotkey, talk, let go -> local Parakeet transcribes and pastes."""

import argparse
import os
import queue
import threading
import time
import winsound

os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")

import keyboard
import numpy as np
import onnx_asr
import pyperclip
import sounddevice as sd

SAMPLE_RATE = 16000
MIN_SECONDS = 0.3  # ignore accidental taps


def beep(freq: int) -> None:
    threading.Thread(target=winsound.Beep, args=(freq, 60), daemon=True).start()


def paste(text: str) -> None:
    try:
        old = pyperclip.paste()
    except Exception:
        old = None
    pyperclip.copy(text)
    time.sleep(0.05)
    keyboard.send("ctrl+v")
    time.sleep(0.2)
    if old is not None:
        pyperclip.copy(old)


class Dictation:
    def __init__(self, model_name: str, quantization: str | None):
        print(f"Loading {model_name} (first run downloads it, ~600MB-2.4GB)...")
        # DirectML = any Windows GPU, no CUDA install needed. Falls back to CPU.
        self.model = onnx_asr.load_model(
            model_name,
            quantization=quantization,
            providers=["DmlExecutionProvider", "CPUExecutionProvider"],
        )
        self.model.recognize(np.zeros(SAMPLE_RATE, dtype=np.float32))  # warm-up
        self.recording = False
        self.chunks: list[np.ndarray] = []
        self.jobs: queue.Queue[np.ndarray] = queue.Queue()
        # Mic stream stays open so the first syllable never gets clipped.
        self.stream = sd.InputStream(
            samplerate=SAMPLE_RATE, channels=1, dtype="float32", callback=self._on_audio
        )
        self.stream.start()
        threading.Thread(target=self._worker, daemon=True).start()

    def _on_audio(self, indata, frames, time_info, status):
        if self.recording:
            self.chunks.append(indata[:, 0].copy())

    def start(self) -> None:
        if self.recording:
            return  # key auto-repeat
        self.chunks = []
        self.recording = True
        beep(880)

    def stop(self) -> None:
        if not self.recording:
            return
        self.recording = False
        beep(440)
        if self.chunks:
            audio = np.concatenate(self.chunks)
            if len(audio) >= MIN_SECONDS * SAMPLE_RATE:
                self.jobs.put(audio)

    def toggle(self) -> None:
        self.stop() if self.recording else self.start()

    def _worker(self) -> None:
        while True:
            audio = self.jobs.get()
            t = time.perf_counter()
            text = self.model.recognize(audio, sample_rate=SAMPLE_RATE).strip()
            print(f"[{time.perf_counter() - t:.2f}s] {text}")
            if text:
                paste(text + " ")


def main() -> None:
    p = argparse.ArgumentParser(description="Local push-to-talk dictation.")
    p.add_argument("--key", default="right ctrl", help='hotkey, e.g. "right ctrl", "f9", "ctrl+alt+space"')
    p.add_argument("--toggle", action="store_true", help="press once to start, again to stop (default: hold to talk)")
    p.add_argument("--model", default="nemo-parakeet-tdt-0.6b-v2", help="any onnx-asr model, e.g. nemo-parakeet-tdt-0.6b-v3 for multilingual")
    p.add_argument("--int8", action="store_true", help="use the int8-quantized model (smaller download, a bit faster on CPU)")
    args = p.parse_args()

    d = Dictation(args.model, "int8" if args.int8 else None)

    if args.toggle:
        keyboard.add_hotkey(args.key, d.toggle)
        how = "press to start/stop"
    else:
        # Hold-to-talk only works with a single key.
        keyboard.on_press_key(args.key, lambda e: d.start())
        keyboard.on_release_key(args.key, lambda e: d.stop())
        how = "hold to talk"

    print(f"Ready. [{args.key}] {how}. Ctrl+C here to quit.")
    try:
        keyboard.wait()
    except KeyboardInterrupt:
        pass
