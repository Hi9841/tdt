"""Isolated Windows tray workflow check. Run with uv run."""

import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "desktop/target/debug/TDT.exe"
u = c.WinDLL("user32", use_last_error=True)
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
        time.sleep(0.05)
    raise AssertionError("Timed out waiting for " + str(fn))


def key(hwnd, value):
    u.PostMessageW(hwnd, 0x100, value, 0)
    u.PostMessageW(hwnd, 0x101, value, 0)
    time.sleep(0.1)


def rect(hwnd):
    value = w.RECT()
    assert u.GetWindowRect(hwnd, c.byref(value))
    return value.left, value.top, value.right, value.bottom


def menu_action(proc, tray, label):
    u.PostMessageW(tray, 6002, 0, 0x204)
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
    time.sleep(0.4)


def launch(state):
    env = dict(os.environ, TDT_PREVIEW_STATE=state, TDT_PREVIEW_TRAY="1", TDT_DISABLE_UPDATES="1")
    proc = subprocess.Popen([str(EXE)], cwd=EXE.parent, env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    hwnd = wait_for(lambda: find(proc.pid, "Zed::Window"))
    tray = wait_for(lambda: find(proc.pid, "tray_icon_app"))
    time.sleep(0.7)
    return proc, hwnd, tray


def main():
    proc, hwnd, tray = launch("bubble.idle")
    try:
        assert not u.IsWindowVisible(hwnd), "Idle should live only in tray"
        u.PostMessageW(tray, 6002, 0, 0x202)
        wait_for(lambda: u.IsWindowVisible(hwnd))
        time.sleep(0.4)
        assert rect(hwnd)[3] - rect(hwnd)[1] > 300, "Tray click opens settings, not a status bubble"
        menu_action(proc, tray, "Hide overlay")
        assert not u.IsWindowVisible(hwnd)
        menu_action(proc, tray, "Keep bubble in tray")
        assert u.IsWindowVisible(hwnd), "Disabling tray mode restores floating bubble"
        menu_action(proc, tray, "Keep bubble in tray")
        assert not u.IsWindowVisible(hwnd), "Enabling tray mode hides idle bubble"
        menu_action(proc, tray, "Open settings")
        assert u.IsWindowVisible(hwnd)
        assert rect(hwnd)[3] - rect(hwnd)[1] > 300
        assert not u.GetWindowLongW(hwnd, -20) & 0x08000000
        key(hwnd, 0x1B)
        wait_for(lambda: not u.IsWindowVisible(hwnd), timeout=7)
        print("PASS: idle hiding, click opens settings, native keyboard menu hide/toggle/settings, Escape return")
    finally:
        proc.terminate()
        proc.wait(timeout=5)

    for state in ["bubble.listening.loud", "bubble.transcribing", "bubble.success.copied", "bubble.error", "bubble.no-speech"]:
        proc, hwnd, tray = launch(state)
        try:
            assert not u.IsWindowVisible(hwnd), state + " must stay entirely in the tray"
            assert u.GetWindowLongW(hwnd, -20) & 0x08000000
            menu_action(proc, tray, "Open settings")
            assert u.IsWindowVisible(hwnd), "Recovery/settings remain explicitly accessible"
            time.sleep(0.3)
            key(hwnd, 0x1B)
            wait_for(lambda: not u.IsWindowVisible(hwnd))
            time.sleep(0.4)
            assert not u.IsWindowVisible(hwnd), "Closing settings must never reveal the status bubble"
            print("PASS:", state, "icon only; explicit settings open and Escape returns to tray")
        finally:
            proc.terminate()
            proc.wait(timeout=5)


if __name__ == "__main__":
    main()
