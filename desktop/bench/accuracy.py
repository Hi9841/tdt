# /// script
# requires-python = ">=3.10"
# dependencies = ["whisper-normalizer"]
# ///
"""Word error rate and speed of bench-asr on folders of clips.

usage: uv run bench/accuracy.py BENCH_EXE SET [SET ...] [--engine native|photon|app] [--show N]

Run from desktop/. SET is a folder under target/accuracy (clean, other, tech, tts16).
WER uses the Whisper English normalizer, as the Open ASR Leaderboard does.
--engine native: TDT's Rust engine on each whole clip (default).
--engine photon: the Python Photon worker on each whole clip.
--engine app: the app's own path (pause cuts and the dictionary file).
Speed is total audio time divided by total transcription time.
"""

import re
import subprocess
import sys
from pathlib import Path

from whisper_normalizer.english import EnglishTextNormalizer

DESKTOP = Path(__file__).resolve().parents[1]
LINE = re.compile(r'^\[([\d.]+)s\] (\S+)\.wav -> ([\d.]+) ms[^:]*: "(.*)"$')
FLAGS = {"native": ["--native"], "photon": ["--photon"], "app": []}
NORMALIZE = EnglishTextNormalizer()


def words(text: str) -> list[str]:
    return NORMALIZE(text).split()


def edits(ref: list[str], hyp: list[str]) -> int:
    prev = list(range(len(hyp) + 1))
    for i, r in enumerate(ref):
        cur = [i + 1] + [0] * len(hyp)
        for j, h in enumerate(hyp):
            cur[j + 1] = min(prev[j] + (r != h), prev[j + 1] + 1, cur[j] + 1)
        prev = cur
    return prev[-1]


def unescape(text: str) -> str:
    return text.replace('\\"', '"').replace("\\'", "'").replace("\\\\", "\\")


def score(exe: str, name: str, engine: str) -> list[tuple[int, int, str, str, str, int, float]]:
    """One row per clip: (errors, reference words, clip, reference, hypothesis, ms, seconds)."""
    folder = DESKTOP / "target" / "accuracy" / name
    out = subprocess.run(
        [exe, "--dir", str(folder), *FLAGS[engine]],
        cwd=DESKTOP, capture_output=True, text=True, encoding="utf-8", check=True,
    ).stdout
    rows = []
    for line in out.splitlines():
        m = LINE.match(line)
        if not m:
            continue
        secs, clip, ms, hyp = float(m.group(1)), m.group(2), float(m.group(3)), unescape(m.group(4))
        # utf-8-sig: PowerShell writes a byte order mark that would count as an error.
        ref = (folder / f"{clip}.txt").read_text(encoding="utf-8-sig")
        r = words(ref)
        rows.append((edits(r, words(hyp)), len(r), clip, ref.strip(), hyp, ms, secs))
    return rows


def main() -> None:
    args = sys.argv[1:]
    engine, show = "native", 0
    if "--engine" in args:
        i = args.index("--engine")
        engine = args[i + 1]
        del args[i : i + 2]
    if "--show" in args:
        i = args.index("--show")
        show = int(args[i + 1])
        del args[i : i + 2]
    if engine not in FLAGS or len(args) < 2:
        sys.exit(__doc__)
    exe, sets = args[0], args[1:]
    for name in sets:
        rows = score(exe, name, engine)
        errors, total = sum(r[0] for r in rows), sum(r[1] for r in rows)
        ms = sorted(r[5] for r in rows)
        median = ms[len(ms) // 2] if ms else 0
        busy = sum(ms) / 1000
        rtfx = sum(r[6] for r in rows) / busy if busy else 0.0
        print(f"{name}: WER {100 * errors / max(total, 1):.2f} % ({errors} errors / {total} words, "
              f"{len(rows)} clips, median {median} ms, {rtfx:.0f}x realtime)")
        for err, _, clip, ref, hyp, *_ in sorted(rows, reverse=True)[:show]:
            print(f"  {err:2d} {clip}\n     ref: {ref}\n     hyp: {hyp}")


if __name__ == "__main__":
    main()
