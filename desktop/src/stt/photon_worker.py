#!/usr/bin/env python3
"""Persistent Photon speech worker for TDT.

Communicates over standard I/O with JSON-lines:
  -> {"cmd": "init", "model_dir": "..."}
  <- {"status": "ready"}
  -> {"cmd": "transcribe", "audio": "<base64_wav>"}
  <- {"status": "ok", "text": "...", "duration": 1.23, "inference_ms": 45.6}
  -> {"cmd": "ping"}
  <- {"status": "pong"}
  -> {"cmd": "shutdown"}
  <- {"status": "bye"}
"""

from __future__ import annotations

import base64
import json
import os
import sys
import time

# Disable third-party network warnings and unnecessary cloud prompts
os.environ["MOONDREAM_API_KEY"] = ""
os.environ["HF_HUB_DISABLE_SYMLINKS_WARNING"] = "1"

# Safe fallback for ternary quantization resident form on CPU platforms:
# When CPU does not have avx512vnni or the native kernel is unbuilt/scalar,
# instead of throwing NotImplementedError, fall back to "dense" oracle form.
try:
    import kestrel_kernels.ternary as kt

    orig_resident_form = getattr(kt, "resident_form", None)
    if orig_resident_form:
        def safe_resident_form(device):
            try:
                return orig_resident_form(device)
            except NotImplementedError:
                # stderr, never stdout: stdout is the JSON protocol.
                print(
                    "native ternary kernel unavailable; falling back to dense",
                    file=sys.stderr,
                    flush=True,
                )
                return "dense"

        kt.resident_form = safe_resident_form
except Exception as exc:  # import failure is fine; only patching is conditional
    print(f"kestrel_kernels patch skipped: {exc}", file=sys.stderr, flush=True)


def main() -> None:
    client = None

    if hasattr(sys.stdin, "reconfigure"):
        sys.stdin.reconfigure(encoding="utf-8")
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            cmd = req.get("cmd")

            if cmd == "init":
                model_dir = req.get("model_dir")
                import moondream as md

                kwargs: dict[str, object] = {"device": "cpu"}
                if model_dir and os.path.isdir(model_dir):
                    kwargs["model_path"] = model_dir
                else:
                    # Fail loudly: without a local model, md.photon would hit
                    # the network, which breaks the offline promise.
                    raise RuntimeError(
                        f"Model directory is missing or not a directory: {model_dir!r}"
                    )

                client = md.photon("moondream/parakeet-redux", **kwargs)

                # Warm-up inference on a tiny silence PCM WAV buffer to pay
                # PyTorch and kernel initialization cost before user speech.
                try:
                    silence_wav = (
                        b"RIFF$\x00\x00\x00WAVEfmt \x10\x00\x00\x00"
                        b"\x01\x00\x01\x00\x80>\x00\x00\x00}\x00\x00"
                        b"\x02\x00\x10\x00data\x00\x00\x00\x00"
                    )
                    client.transcribe(audio=silence_wav)
                except Exception:
                    pass

                print(json.dumps({"status": "ready"}), flush=True)

            elif cmd == "transcribe":
                if client is None:
                    print(
                        json.dumps(
                            {"status": "error", "message": "Model not initialized"}
                        ),
                        flush=True,
                    )
                    continue

                audio_b64 = req.get("audio", "")
                if not audio_b64:
                    print(
                        json.dumps(
                            {"status": "ok", "text": "", "duration": 0.0, "inference_ms": 0.0}
                        ),
                        flush=True,
                    )
                    continue

                audio_bytes = base64.b64decode(audio_b64)
                t0 = time.perf_counter()
                res = client.transcribe(audio=audio_bytes)
                t1 = time.perf_counter()

                text = res.get("text", "") if isinstance(res, dict) else str(res)
                duration = (
                    res.get("duration_seconds", 0.0) if isinstance(res, dict) else 0.0
                )
                inference_ms = round((t1 - t0) * 1000, 2)

                print(
                    json.dumps(
                        {
                            "status": "ok",
                            "text": text.strip(),
                            "duration": duration,
                            "inference_ms": inference_ms,
                        }
                    ),
                    flush=True,
                )

            elif cmd == "ping":
                print(json.dumps({"status": "pong"}), flush=True)

            elif cmd == "shutdown":
                print(json.dumps({"status": "bye"}), flush=True)
                break

            else:
                print(
                    json.dumps(
                        {"status": "error", "message": f"Unknown command: {cmd}"}
                    ),
                    flush=True,
                )

        except Exception as exc:
            print(
                json.dumps({"status": "error", "message": str(exc)}),
                flush=True,
            )


if __name__ == "__main__":
    main()
