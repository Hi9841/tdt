# Speech benchmarks

These files measure the word error rate (WER) and the speed of TDT's speech
engines. The results are in the main [README](../../README.md#benchmarks), and
the raw output is in [results-2026-10-20.txt](results-2026-10-20.txt).

## Test sets

| Set | Clips | Source |
|---|---|---|
| `clean` | 200 | LibriSpeech test-clean: every 13th of the 2620 utterances |
| `other` | 200 | LibriSpeech test-other: every 14th of the 2939 utterances |
| `tech` | 40 | `tech-corpus.txt`: sentences with developer terms, spoken by the Windows voices |
| `tts16` | 16 | `corpus.txt`: everyday dictation, spoken by the Windows voices |
| `names` | 32 | `names-corpus.txt`: names and product names (`names-terms.txt`), spoken by the Windows voices |
| `names-neg` | 24 | `names-negative.txt`: ordinary words that sound like those names |
| `accent` | 160 | `corpus.txt`, `commands-corpus.txt`, and `names-corpus.txt`, spoken by five non-English Piper voices (`accent.py`) |

LibriSpeech is recorded human speech. `tech` and `tts16` are synthetic speech
from the Microsoft David and Zira voices. Other voices give other audio, so
results for these two sets can be different on another computer.

## How the score is calculated

- **WER:** `accuracy.py` normalizes the reference and the transcript with the
  Whisper English normalizer, as the Open ASR Leaderboard does. Then it counts
  the word substitutions, insertions and deletions.
- **Speed (× realtime):** the total audio time divided by the total
  transcription time. The bench gives each whole clip to the engine in one
  call. The model loads before the first clip, so the load time is not in the
  result.
- The bench cuts the silence after the last word to 150 ms, the same tail that
  the app keeps when a recording stops.

## Reproduce

Run these commands from `desktop/` in PowerShell:

```powershell
# 1. Make the test sets.
uv run bench/librispeech.py
powershell -ExecutionPolicy Bypass -File bench/make-clips.ps1 -Corpus bench/tech-corpus.txt -OutDir target/accuracy/tech
powershell -ExecutionPolicy Bypass -File bench/make-clips.ps1 -OutDir target/accuracy/tts16
powershell -ExecutionPolicy Bypass -File bench/make-clips.ps1 -Corpus bench/names-corpus.txt -OutDir target/accuracy/names
powershell -ExecutionPolicy Bypass -File bench/make-clips.ps1 -Corpus bench/names-negative.txt -OutDir target/accuracy/names-neg
uv run bench/accent.py <piper voices dir> target/accuracy/accent bench/corpus.txt bench/commands-corpus.txt bench/names-corpus.txt

# 2. Build the bench.
cargo build --release --bin bench-asr
$bench = "target/release/bench-asr.exe"

# 3. Score each engine.
uv run bench/accuracy.py $bench clean other tech tts16 --engine native

# 4. The dictionary on names, and on words that only sound like them.
uv run bench/terms.py $bench bench/names-terms.txt names names-neg --show
```

For the v0.2.25 column, check out tag `v0.2.25`, build the bench, and run the
`--engine native` line. Add `--show 10` to see the clips with the most errors.
`--engine app` scores the app's own path, which also cuts long recordings at
pauses and reads the user's dictionary file.

## Same model on other runtimes

`runtimes.py` runs each model on every runtime that supports it, on the same
clips and with the same scorer: Trit and Photon for Parakeet Redux; ONNX
Runtime and sherpa-onnx for Parakeet TDT; faster-whisper, whisper.cpp,
Transformers and OpenAI Whisper for Whisper; Transformers and sherpa-onnx for
Moonshine. Each engine runs in its own process and its own uv environment.

```powershell
cargo build --release -p trit-ffi --manifest-path ../trit/Cargo.toml   # Trit for Python
uv run --no-project --python 3.12 python bench/runtimes.py --list      # engines and models
uv run --no-project --python 3.12 python bench/runtimes.py parakeet-redux whisper-turbo
uv run --no-project --python 3.12 python bench/runtimes.py all         # about 2 hours
python bench/runtimes_report.py                                        # ../docs/engine-comparison.html
```

Results append to `target/runtimes/results.jsonl` (the last line of each
engine wins), and each engine's transcripts go next to it. The committed run
is [runtimes-results.jsonl](runtimes-results.jsonl). GPU engines need an
NVIDIA GPU with CUDA 12.

## Scripts

| Script | What it does |
|---|---|
| `terms.py` | The dictionary: names spelled right, names put where nobody said them, and word errors, with and without a dictionary file |
| `accent.py` | Speaks corpora with non-English Piper voices (the `accent` set) |
| `accuracy.py` | Errors and speed of the TDT model through `bench-asr` (the ratchet in `engine-budget.json`) |
| `librispeech.py` | Writes the 200 LibriSpeech clips per set as 16 kHz WAV and text |
| `runtimes.py` | The same model on every runtime (above) |
| `cleanup_runtimes.py` | S1-mini on Trit, Transformers, and llama.cpp, scored the same way |
| `runtimes_table.py` | Errors per model and runtime from `target/runtimes/results.jsonl`, as text |
| `wer_rule.py` | Exits 1 when Trit's default makes more errors than another runtime on a model |
| `runtimes_report.py` | Writes `docs/engine-comparison.{md,html}` and `benchmarks.json` for the site |
| `readme_charts.py` | Draws the README charts in `docs/images/` from `benchmarks.json` |
| `memory.py` | Memory and speed of one Trit model over N clips |
