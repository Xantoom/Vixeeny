# Testing the beta

Vixeeny 0.9 is the first public beta. Windows is the most tested; macOS and Linux were built
and checked by automated tests only, so **reports from real machines are what we need**.

## What to try

1. Install it (see the [README](../README.md)) and walk through the first-run wizard.
2. Take a region screenshot, a window screenshot and a full-screen one; annotate one, copy it
   and paste it into another application.
3. Record 30 seconds with system audio, pause, resume, stop; play the file.
4. Read the text of a zone (OCR).
5. Quit from the tray icon; start it again; try "Start with the session".

## Especially wanted

- **Linux**: GNOME (Wayland), KDE Plasma (Wayland and X11), Hyprland / Sway, and any X11 desktop.
  Which package did you use? Did the tray icon appear? Did the shortcuts work, or did you need
  `vixeeny-daemon ctl`? Did the portal ask for the screen each time?
- **macOS**: the permission prompts (screen recording, microphone), the menu-bar icon, the
  Finder Quick Action.
- **Hardware encoders**: which GPU, which encoder was chosen (Settings → Hardware), did the
  recording play correctly.

## Reporting

Open an [issue](https://github.com/Xantoom/Vixeeny/issues/new/choose) — "Bug report" for a
problem, "Beta feedback" to say how it went (including "everything worked"). Paste the output of
**Settings → About → Copy system info** (or `vixeeny-app --system-info` in a terminal): it lists
the version, system, displays, GPUs and encoders, and nothing personal.
