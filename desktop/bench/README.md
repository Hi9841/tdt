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

# 2. Build the bench.
cargo build --release --bin bench-asr
$bench = "target/release/bench-asr.exe"

# 3. Score each engine.
uv run bench/accuracy.py $bench clean other tech tts16 --engine native
$env:TDT_DICTIONARY = "assets/dictionary/developer-terms.txt"
uv run bench/accuracy.py $bench clean other tech tts16 --engine native
Remove-Item Env:TDT_DICTIONARY
```

For the v0.2.25 column, check out tag `v0.2.25`, build the bench, and run the
`--engine native` line. Add `--show 10` to see the clips with the most errors.
`--engine app` scores the app's own path, which also cuts long recordings at
pauses and reads the user's dictionary file.
