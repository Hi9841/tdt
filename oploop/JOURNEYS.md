# OpLoop journeys: TDT app

| Journey | Start and end | Measure |
|---|---|---|
| Wait after the stop | The microphone closes, the model text is ready | `bench-asr --dir <36 real takes> --whole --realtime`: mean, p90, max |
| Same under load | As above, with 16 workers on CCD0 | The same, with the load started first |
| Same on a slow CPU | As above | `TRIT_THREADS=2`, and `TRIT_THREADS=4` with `TRIT_ISA=avx2` |

Quality is the hard gate: a change that loses text does not stay, whatever it saves. Score long
takes with true text, paired per take, with the normalizer of `bench/accuracy.py`.
