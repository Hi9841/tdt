"""Isolated Windows tray workflow check. Run with uv run.

Drives the real tray icon and its native menu through window messages, in
preview mode, so it runs beside an installed TDT without touching it.
"""

import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
EXE = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "desktop/target/debug/TDT.exe"
u = c.WinDLL("user32", use_last_error=True)
g = c.WinDLL("gdi32", use_last_error=True)
u.SetProcessDPIAware()
CALLBACK = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
u.EnumWindows.argtypes = [CALLBACK, w.LPARAM]
u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
u.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.IsWindowVisible.argtypes = [w.HWND]
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.SendMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.SendMessageW.restype = c.c_ssize_t
u.GetMenuStringW.argtypes = [w.HMENU, w.UINT, w.LPWSTR, c.c_int, w.UINT]
u.GetMenuItemCount.argtypes = [w.HMENU]
u.GetMenuState.argtypes = [w.HMENU, w.UINT, w.UINT]
u.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
u.GetWindowLongW.argtypes = [w.HWND, c.c_int]
u.GetDC.restype = w.HDC
g.GetPixel.argtypes = [w.HDC, c.c_int, c.c_int]

TRAY_MESSAGE = 6002
WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_RBUTTONUP = 0x202, 0x203, 0x205
WS_EX_NOACTIVATE = 0x08000000


def find(pid, name):
    result = []

    @CALLBACK
    def visit(hwnd, _):
        owner = w.DWORD()
        u.GetWindowThreadProcessId(hwnd, c.byref(owner))
        label = c.create_unicode_buffer(128)
        u.GetClassNameW(hwnd, label, 128)
        if owner.value == pid and label.value == name:
            result.append(hwnd)
        return True

    u.EnumWindows(visit, 0)
    return result[0] if result else None


def wait_for(fn, timeout=10):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        result = fn()
        if result:
            return result
        time.sleep(0.02)
    raise AssertionError("Timed out waiting for " + str(fn))


def key(hwnd, value):
    u.PostMessageW(hwnd, 0x100, value, 0)
    u.PostMessageW(hwnd, 0x101, value, 0)
    time.sleep(0.1)


def rect(hwnd):
    value = w.RECT()
    assert u.GetWindowRect(hwnd, c.byref(value))
    return value.left, value.top, value.right, value.bottom


def panel_open(hwnd):
    """The open panel takes focus, so the overlay drops WS_EX_NOACTIVATE."""
    return not u.GetWindowLongW(hwnd, -20) & WS_EX_NOACTIVATE


def panel_fully_drawn(hwnd):
    """Near the top of the window, the full panel is opaque black; the
    resting pill or a panel still growing leaves the desktop showing."""
    left, top, right, _ = rect(hwnd)
    screen = u.GetDC(None)
    try:
        samples = [g.GetPixel(screen, x, top + 12) for x in range(left + 40, right - 40, 40)]
    finally:
        u.ReleaseDC(None, screen)
    return all(color == 0 for color in samples)


def menu_action(proc, tray, label):
    u.PostMessageW(tray, TRAY_MESSAGE, 0, WM_RBUTTONUP)
    menu_window = wait_for(lambda: find(proc.pid, "#32768"))
    menu = u.SendMessageW(menu_window, 0x1E1, 0, 0)
    labels = []
    for index in range(u.GetMenuItemCount(menu)):
        text = c.create_unicode_buffer(256)
        u.GetMenuStringW(menu, index, text, 256, 0x400)
        if text.value:
            labels.append(text.value)
    assert label in labels, labels
    # Drive the real native menu through keyboard messages, including selection.
    for _ in range(labels.index(label) + 1):
        key(tray, 0x28)
    selected = [i for i in range(u.GetMenuItemCount(menu)) if u.GetMenuState(menu, i, 0x400) & 0x80]
    assert len(selected) == 1, "Keyboard must highlight one native menu item"
    selected_label = c.create_unicode_buffer(256)
    u.GetMenuStringW(menu, selected[0], selected_label, 256, 0x400)
    assert selected_label.value == label, selected_label.value
    key(tray, 0x0D)


def launch(state):
    env = dict(os.environ, TDT_PREVIEW_STATE=state, TDT_PREVIEW_TRAY="1", TDT_DISABLE_UPDATES="1")
    proc = subprocess.Popen([str(EXE)], cwd=EXE.parent, env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    hwnd = wait_for(lambda: find(proc.pid, "Zed::Window"))
    tray = wait_for(lambda: find(proc.pid, "tray_icon_app"))
    time.sleep(1.0)
    return proc, hwnd, tray


def close_with_escape(hwnd):
    key(hwnd, 0x1B)
    wait_for(lambda: not panel_open(hwnd), timeout=3)


def main():
    proc, hwnd, tray = launch("bubble.idle")
    try:
        size = rect(hwnd)
        assert u.IsWindowVisible(hwnd), "The resting pill is always on screen"
        assert not panel_open(hwnd)

        u.PostMessageW(tray, TRAY_MESSAGE, 0, WM_LBUTTONUP)
        wait_for(lambda: panel_open(hwnd), timeout=3)
        wait_for(lambda: panel_fully_drawn(hwnd), timeout=3)
        print("PASS: a tray click opens Settings")

        # A double click arrives as click, double click, click. The open panel
        # must stay fully drawn: a restarted morph would shrink it first.
        for message in (WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_LBUTTONUP):
            u.PostMessageW(tray, TRAY_MESSAGE, 0, message)
        for _ in range(15):
            time.sleep(0.02)
            assert panel_fully_drawn(hwnd), "A double click must not restart the morph"
        print("PASS: a tray double click keeps Settings still")

        close_with_escape(hwnd)
        print("PASS: Escape closes Settings back to the pill")

        menu_action(proc, tray, "Open settings")
        wait_for(lambda: panel_open(hwnd), timeout=3)
        close_with_escape(hwnd)
        print("PASS: the tray menu opens Settings")

        assert rect(hwnd) == size, "The overlay window never resizes"
        print("PASS: the window kept one size the whole time")
    finally:
        proc.terminate()
        proc.wait(timeout=5)


if __name__ == "__main__":
    main()
