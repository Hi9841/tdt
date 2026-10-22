"""Captures the app states of the README and the website as transparent PNGs,
and composes the usage example from them.

usage (from the repository root, Windows, after
`cargo build --release --manifest-path desktop/Cargo.toml --bin TDT`):

  uv run --no-project --python 3.12 --with pillow --with numpy python docs/images/capture.py [STATE ...]

For each state, TDT starts frozen in it (`TDT_PREVIEW_STATE`, see
desktop/src/ui/preview.rs), with its own config and no microphone, hotkey,
or tray. The script moves the window to a free part of the screen, shows it
over a black and then a white backdrop, and computes the alpha of each pixel
from the two captures: the shadows and the rounded corners stay exact on any
page. Do not use the mouse or the keyboard while it runs (about 6 s a state).
"""

import ctypes
import os
import shutil
import subprocess
import sys
import time
import tkinter
from ctypes import wintypes
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont, ImageGrab

ROOT = Path(__file__).resolve().parents[2]
APP = ROOT / "desktop" / "target" / "release" / "TDT.exe"
OUT = Path(__file__).resolve().parent / "app"
SITE = ROOT / "site" / "assets" / "tdt-page" / "app"  # the website uses the same files
# State -> file name. The README and the site use these names.
STATES = {
    "bubble.idle": "pill-idle",
    "bubble.listening.loud": "pill-listening",
    "bubble.transcribing": "pill-transcribing",
    "bubble.success.pasted": "pill-pasted",
    "panel.stats.history": "panel-history",
    "panel.settings.defaults": "panel-settings",
}
PLACE = (160, 160)  # top-left of the window during capture, away from a running TDT's pill
MARGIN = 48  # backdrop around the window, for the shadow
user32 = ctypes.windll.user32
user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]


def windows_of(pid: int) -> list[tuple[int, tuple[int, int, int, int]]]:
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def visit(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            rect = wintypes.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            if rect.right - rect.left > 20 and rect.bottom - rect.top > 20:
                found.append((hwnd, (rect.left, rect.top, rect.right, rect.bottom)))
        return True

    user32.EnumWindows(visit, 0)
    return found


def capture(state: str, backdrop: tkinter.Tk) -> Image.Image:
    env = {**os.environ, "TDT_PREVIEW_STATE": state}
    app = subprocess.Popen([str(APP)], env=env)
    try:
        deadline = time.time() + 15
        while not windows_of(app.pid):
            if time.time() > deadline:
                sys.exit(f"{state}: no TDT window after 15 s")
            time.sleep(0.2)
        time.sleep(2.0)  # open animations
        hwnd, (left, top, right, bottom) = max(windows_of(app.pid), key=lambda w: (w[1][2] - w[1][0]) * (w[1][3] - w[1][1]))
        width, height = right - left, bottom - top
        # HWND_TOPMOST, SWP_NOSIZE | SWP_NOACTIVATE
        user32.SetWindowPos(hwnd, -1, PLACE[0], PLACE[1], 0, 0, 0x0001 | 0x0010)
        box = (PLACE[0] - MARGIN, PLACE[1] - MARGIN, PLACE[0] + width + MARGIN, PLACE[1] + height + MARGIN)
        backdrop.geometry(f"{box[2] - box[0]}x{box[3] - box[1]}+{box[0]}+{box[1]}")
        shots = []
        for color in ("#000000", "#ffffff"):
            backdrop.configure(bg=color)
            backdrop.deiconify()
            backdrop.update()
            user32.SetWindowPos(hwnd, -1, PLACE[0], PLACE[1], 0, 0, 0x0001 | 0x0010)
            time.sleep(0.8)
            shots.append(np.asarray(ImageGrab.grab(bbox=box), dtype=np.float32)[..., :3])
        backdrop.withdraw()
        backdrop.update()
    finally:
        app.kill()
    black, white = shots
    alpha = np.clip(1.0 - (white - black).mean(axis=2) / 255.0, 0.0, 1.0)
    color = np.where(alpha[..., None] > 0.004, black / np.maximum(alpha[..., None], 0.004), 0.0)
    rgba = np.dstack([np.clip(color, 0, 255), alpha * 255]).astype(np.uint8)
    image = Image.fromarray(rgba, "RGBA")
    solid = np.argwhere(alpha > 0.03)
    (y0, x0), (y1, x1) = solid.min(axis=0), solid.max(axis=0)
    pad = 4
    return image.crop((max(0, x0 - pad), max(0, y0 - pad), min(image.width, x1 + pad + 1), min(image.height, y1 + pad + 1)))


def usage_example() -> Image.Image:
    """The three steps of a dictation, from the pill captures: usage-example.png."""
    fonts = Path("C:/Windows/Fonts")
    font = lambda name, size: ImageFont.truetype(str(fonts / name), size)
    regular, semibold, small, keys = font("segoeui.ttf", 17), font("seguisb.ttf", 17), font("segoeui.ttf", 14), font("seguisb.ttf", 14)
    pill = lambda name: Image.open(OUT / f"{name}.png")
    ink, muted, line, card, accent = "#f2f3f7", "#9aa0b1", "#2a2e3a", "#161922", "#3b82f6"
    image = Image.new("RGBA", (920, 400), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((0, 0, 919, 399), 18, fill="#0d0f14", outline=line)

    # Left: the steps, each with its pill.
    steps = [
        ("1", "Hold", "and speak", "pill-listening"),
        ("2", "Release. Trit transcribes", "on your computer", "pill-transcribing"),
        ("3", "The text is typed", "where your cursor is", "pill-pasted"),
    ]
    y = 40
    for number, title, detail, name in steps:
        draw.ellipse((32, y, 58, y + 26), fill=accent)
        draw.text((45, y + 13), number, font=keys, fill="#ffffff", anchor="mm")
        draw.text((72, y + 13), title, font=semibold, fill=ink, anchor="lm")
        if number == "1":
            x = 72 + draw.textlength(title, font=semibold) + 10
            for key in ("Ctrl", ";"):
                width = draw.textlength(key, font=keys) + 18
                draw.rounded_rectangle((x, y, x + width, y + 26), 6, fill="#232734", outline="#3a3f4e")
                draw.text((x + width / 2, y + 13), key, font=keys, fill=ink, anchor="mm")
                x += width + 6
        draw.text((72, y + 40), detail, font=small, fill=muted, anchor="lm")
        if name != "pill-pasted":
            shot = pill(name)
            image.alpha_composite(shot, (330 - shot.width // 2 + 0, y + 20 - shot.height // 2))
        y += 112

    # Right: the app the text goes to, with the pasted pill under it.
    left, top, right, bottom = 400, 32, 888, 300
    draw.rounded_rectangle((left, top, right, bottom), 12, fill=card, outline=line)
    draw.text((left + 18, top + 20), "Notes", font=small, fill=muted, anchor="lm")
    for i, glyph in enumerate(("–", "□", "×")):  # minimize, maximize, close
        draw.text((right - 88 + i * 32, top + 20), glyph, font=small, fill=muted, anchor="mm")
    draw.line((left, top + 40, right, top + 40), fill=line)
    draw.text((left + 24, top + 68), "Ops checklist", font=semibold, fill=ink, anchor="lm")
    draw.text((left + 24, top + 104), "The deploy finished at 14:20.", font=regular, fill=muted, anchor="lm")
    text = "Restart the Kubernetes pod, then check the PostgreSQL logs."
    words, rows, row = text.split(), [], ""
    for word in words:
        if draw.textlength(f"{row} {word}".strip(), font=regular) > right - left - 56:
            rows.append(row)
            row = word
        else:
            row = f"{row} {word}".strip()
    rows.append(row)
    ty = top + 140
    for i, row in enumerate(rows):
        width = draw.textlength(row, font=regular)
        draw.rounded_rectangle((left + 18, ty - 15, left + 30 + width, ty + 15), 6, fill="#1d2a44")
        draw.text((left + 24, ty), row, font=regular, fill=ink, anchor="lm")
        if i == len(rows) - 1:
            draw.line((left + 28 + width, ty - 11, left + 28 + width, ty + 11), fill=accent, width=2)
        ty += 34
    shot = pill("pill-pasted")
    image.alpha_composite(shot, ((left + right) // 2 - shot.width // 2, bottom + 24))
    return image


def main() -> None:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)  # physical pixels everywhere
    if not APP.exists():
        sys.exit(f"{APP} is missing. Build it: cargo build --release --manifest-path desktop/Cargo.toml --bin TDT")
    states = sys.argv[1:] or list(STATES)
    OUT.mkdir(parents=True, exist_ok=True)
    backdrop = tkinter.Tk()
    backdrop.overrideredirect(True)
    backdrop.attributes("-topmost", True)
    backdrop.withdraw()
    for state in states:
        image = capture(state, backdrop)
        path = OUT / f"{STATES.get(state, state)}.png"
        image.save(path, optimize=True)
        print(f"{state}: {path.relative_to(ROOT)} {image.width}x{image.height}")
    backdrop.destroy()
    if all((OUT / f"{name}.png").exists() for name in ("pill-listening", "pill-transcribing", "pill-pasted")):
        usage_example().save(OUT / "usage-example.png", optimize=True)
        print(f"usage: {(OUT / 'usage-example.png').relative_to(ROOT)}")
    SITE.mkdir(parents=True, exist_ok=True)
    for image in OUT.glob("*.png"):
        shutil.copyfile(image, SITE / image.name)
    print(f"copied to {SITE.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
