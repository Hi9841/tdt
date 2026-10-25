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

## Engine benchmarks

The engine benchmarks are in the Trit repository, in
[bench/](https://github.com/Hi9841/trit/tree/main/bench): the same model on every CPU runtime
(`runtimes.py`), the S1-mini cleanup comparison, the memory check, and the
text-to-speech check against Transformers (`tts.py`). The report is
[docs/engine-comparison.md](https://github.com/Hi9841/trit/blob/main/docs/engine-comparison.md).
This folder keeps the checks of the app: the ratchet, the dictionary, and
the accent set.

## Scripts

| Script | What it does |
|---|---|
| `terms.py` | The dictionary: names spelled right, names put where nobody said them, and word errors, with and without a dictionary file |
| `accent.py` | Speaks corpora with non-English Piper voices (the `accent` set) |
| `accuracy.py` | Errors and speed of the TDT model through `bench-asr` (the ratchet in `engine-budget.json`) |
| `librispeech.py` | Writes the 200 LibriSpeech clips per set as 16 kHz WAV and text |
