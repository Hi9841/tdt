# Overlay comparison

Committed direction: quiet utility. Chosen after comparing the capsule, the quiet utility, and the stack.

Z-order is not one of these shapes. The pill is raised again inside the existing topmost band when an ordinary app or the taskbar covers it, and it still yields to the Start menu, Alt-Tab, menus, and the tray overflow.

## Capsule

The pill is one fully rounded strip. The left side is a colored status word (`Ready` in green, `Listening` in blue) with no lamp. The center is the shortcut, or a waveform that stays up through transcription. The right side is `Hide` and `Settings`, and `Stop` is the same quiet button as `Hide`.

Settings is a tab next to History. History opens on a three-count grid (transcriptions, words, recorded time) and the transcript list comes after the counts. Settings is one scroll: a Dictation group, then a Model block, then a Language block, then an App group, then a separate Updates section.

## Quiet utility

The pill is three zones in one short bar: a 6px lamp plus the status word, a center slot, and the actions. Idle shows the shortcut in the slot and `Hide` with `Settings…`. Listening puts the accent waveform in the slot and a filled `Stop` on the right. Transcribing replaces the waveform with a short activity mark. A partial, a result, or the real error text takes the slot when there is something to read.

The panel still has History and Settings tabs. History leads with the latest transcript, and the counts are one line under it. Settings is three groups in task order: Dictation (shortcut, auto-paste), Recognition (model, language), App (start with Windows, updates).

## Stack

The overlay is a narrow vertical card, not a bar. A waveform band spans the top. Status and the partial stack under it. One full-width button is the only action (`Ctrl+;` or `Stop`). There is no Hide control and no left/center/right split.

Opening details does not morph that card into a tabbed panel. Three questions replace the panel: how to dictate, what hears you, and when the app runs. History is not a tab.

## Choice

Quiet utility is the one to ship.

The capsule and the quiet utility are both short bars, but they are not the same layout. The capsule spends the center on a waveform and leads History with counts. The quiet utility spends the center on the words you just said and leads History with those words. Dictation, recognition, and app stay in that order, which is the order of the job.

The stack is a different object. It covers more of the window you are typing into, drops Hide, and replaces the panel with a questionnaire. That fights the loop of talking into another app and glancing at a thin status.

Paste safety from 0.2.9 and quiet-speech transcription from 0.2.10 stay. The shipped pill and settings in `desktop/src/ui/hud.rs` are this quiet utility: lamp, `Settings…`, filled Stop, History transcript first, and the Dictation / Recognition / App groups. The capsule and the stack are these sketches only.
