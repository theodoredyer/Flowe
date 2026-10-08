# Downloads the Parakeet TDT 0.6B v2 ONNX export (~2.4 GB) into %LOCALAPPDATA%\Fleow\model.
# Fleow looks for the model there at startup.
$ErrorActionPreference = "Stop"
$dir = Join-Path $env:LOCALAPPDATA "Fleow\model"
New-Item -ItemType Directory -Force $dir | Out-Null
$base = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main"
foreach ($f in "vocab.txt", "decoder_joint-model.onnx", "encoder-model.onnx", "encoder-model.onnx.data") {
    $dest = Join-Path $dir $f
    if (Test-Path $dest) { "already have $f"; continue }
    "downloading $f ..."
    # BITS shows progress and resumes; fall back to a plain download if BITS is unavailable.
    try { Start-BitsTransfer -Source "$base/$f" -Destination $dest }
    catch { Invoke-WebRequest -Uri "$base/$f" -OutFile $dest }
}
"done -> $dir"
