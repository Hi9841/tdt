# /// script
# requires-python = ">=3.10,<3.13"
# dependencies = ["piper-tts", "numpy", "scipy"]
# ///
"""English sentences spoken by French, Spanish, German, Italian, and
Portuguese Piper voices: accented speech that pushes a multilingual model
into other languages. Each voice speaks every second line.

usage: uv run bench/accent.py VOICES_DIR OUT_DIR CORPUS [CORPUS ...]

VOICES_DIR holds Piper voices (.onnx and .onnx.json) from
https://huggingface.co/rhasspy/piper-voices, for example fr_FR-siwis-medium,
es_ES-davefx-medium, de_DE-thorsten-medium, it_IT-riccardo-x_low, and
pt_BR-faber-medium. A CORPUS line is "id|text" or plain text.
"""

import sys
import wave
from pathlib import Path

import numpy as np
from piper import PiperVoice
from scipy.signal import resample_poly


def main() -> None:
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    voices_dir, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    lines = []
    for corpus in sys.argv[3:]:
        for line in Path(corpus).read_text(encoding="utf-8-sig").splitlines():
            text = line.split("|", 1)[-1].strip()
            if text:
                lines.append(text)
    count = 0
    for v, path in enumerate(sorted(voices_dir.glob("*.onnx"))):
        voice = PiperVoice.load(str(path))
        tag = path.stem.split("-")[0]
        for i, text in enumerate(lines):
            if (i + v) % 2:
                continue
            audio = np.concatenate(
                [np.frombuffer(c.audio_int16_bytes, dtype=np.int16) for c in voice.synthesize(text)]
            ).astype(np.float32)
            audio = resample_poly(audio, 16000, voice.config.sample_rate)
            with wave.open(str(out / f"{tag}-{i:03d}.wav"), "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(16000)
                w.writeframes(np.clip(audio, -32768, 32767).astype(np.int16).tobytes())
            (out / f"{tag}-{i:03d}.txt").write_text(text, encoding="utf-8")
            count += 1
    print(f"wrote {count} clips to {out}")


if __name__ == "__main__":
    main()
