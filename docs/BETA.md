# Testing the beta

Vixeeny is in public beta on Windows 10 and 11. Automated tests cover a lot, but **reports from
real machines are what we need**: other GPUs, other monitor layouts, other scaling factors.

## What to try

1. Install it (see the [README](../README.md)) and walk through the first-run wizard.
2. Take a region screenshot, a window screenshot and a full-screen one, on each monitor if you
   have several; annotate one, copy it and paste it into another application.
3. Record 30 seconds with system audio, pause, resume, stop; play the file.
4. Quit from the tray icon; start it again; try "Start with Windows".
5. When an update comes out: let it install by itself, or install it from Settings → Updates.

## Especially wanted

- **Several monitors** with different scaling factors (100 %, 150 %, 200 %).
- **Hardware encoders**: which GPU, which encoder was chosen (Settings → Video), did the
  recording play correctly.
- **HDR screens**: do the screenshots look right.

## Reporting

Open an [issue](https://github.com/Xantoom/Vixeeny/issues/new/choose) — "Bug report" for a
problem, "Beta feedback" to say how it went (including "everything worked"). For a problem,
**Settings → About → Report a problem** opens the bug form with the system info (version,
system, displays, GPUs, encoders) filled in and selects a report file with the logs of the last
days: drop it in the form. Neither holds your user name or folders.
