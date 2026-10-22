# /// script
# requires-python = ">=3.10"
# dependencies = ["whisper-normalizer"]
# ///
"""How well the dictionary works: terms spelled right, and terms put where they were not said.

usage: uv run bench/terms.py BENCH_EXE DICTIONARY SET [SET ...] [--show]

Run from desktop/. SET is a folder under target/accuracy. Each clip runs through
`bench-asr --native` twice, without and with DICTIONARY (one term per line).
- hits: terms in the reference that the transcript spells exactly (case too).
- false: terms in the transcript that are not in the reference.
- errors: word errors after the Whisper English normalizer.
- changed: clips whose text the dictionary changed.
"""

import os
import re
import subprocess
import sys
from pathlib import Path

from accuracy import DESKTOP, LINE, edits, unescape, words


def run(exe: str, folder: Path, dictionary: str | None) -> dict[str, str]:
    env = dict(os.environ)
    env.pop("TDT_DICTIONARY", None)
    if dictionary:
        env["TDT_DICTIONARY"] = dictionary
    out = subprocess.run(
        [exe, "--dir", str(folder), "--native"],
        cwd=DESKTOP, capture_output=True, text=True, encoding="utf-8", check=True, env=env,
    ).stdout
    return {m.group(2): unescape(m.group(4)) for m in map(LINE.match, out.splitlines()) if m}


def found(term: str, text: str) -> int:
    return len(re.findall(rf"(?<![\w-]){re.escape(term)}(?![\w-])", text))


def main() -> None:
    args = sys.argv[1:]
    show = "--show" in args
    args = [a for a in args if a != "--show"]
    if len(args) < 3:
        sys.exit(__doc__)
    exe, dictionary, sets = args[0], str(Path(args[1]).resolve()), args[2:]
    terms = [t.strip() for t in Path(dictionary).read_text(encoding="utf-8").splitlines()]
    terms = [t for t in terms if t and not t.startswith("#")]
    # A longer term that contains a shorter one counts once, as the longer.
    terms.sort(key=len, reverse=True)
    for name in sets:
        folder = DESKTOP / "target" / "accuracy" / name
        plain, biased = run(exe, folder, None), run(exe, folder, dictionary)
        totals = {"plain": [0, 0, 0], "dict": [0, 0, 0]}
        said = changed = 0
        for clip in sorted(plain):
            ref = (folder / f"{clip}.txt").read_text(encoding="utf-8-sig").strip()
            rest = ref
            wanted = []
            for term in terms:
                n = found(term, rest)
                wanted += [term] * n
                rest = re.sub(rf"(?<![\w-]){re.escape(term)}(?![\w-])", " ", rest)
            said += len(wanted)
            for key, hyp in (("plain", plain[clip]), ("dict", biased[clip])):
                left = hyp
                hits = 0
                for term in wanted:
                    if found(term, left):
                        hits += 1
                        left = re.sub(rf"(?<![\w-]){re.escape(term)}(?![\w-])", " ", left, count=1)
                false = sum(found(t, left) for t in terms if t not in wanted)
                t = totals[key]
                t[0] += hits
                t[1] += false
                t[2] += edits(words(ref), words(hyp))
            if plain[clip] != biased[clip]:
                changed += 1
                if show:
                    print(f"  {clip}\n     ref:  {ref}\n     was:  {plain[clip]}\n     now:  {biased[clip]}")
        for key, (hits, false, errors) in totals.items():
            print(f"{name} {key:5}: hits {hits}/{said}, false {false}, errors {errors}")
        print(f"{name}: dictionary changed {changed}/{len(plain)} clips")


if __name__ == "__main__":
    main()
