# vixeeny-overlay

What Vixeeny draws natively. The zone editor of Print Screen: one borderless window per monitor
whose content is a DirectComposition visual tree. The frozen screen is uploaded once and never drawn again; the
veil, the zone's border and handles are visuals moved by the compositor; only the small pieces
that change (toolbar, style panel, magnifier, labels) are drawn, with Direct2D and DirectWrite.
Nothing presents frames the way a game does, so the screen keeps its refresh rate and the GPU
idles while the pointer rests.

Entry point: `Overlay` (same flow as the editor `Session` of `vixeeny-editor`).

The windows shown often are drawn the same way, on `popup`: the side strip (`side`), the
notification cards (`toast`) and the recording widget (`widget`). Their layout and what input
does to them are plain code the tests drive; the tests also draw them off screen
(`VIXEENY_SCREENSHOTS=<dir>` writes the PNGs).

`cargo run --example smoke -- out.png` shows the editor in a small window for about two
seconds, draws a zone, opens the style panel and saves what the screen shows: the check that
the visual tree really composes (the tests run with hidden windows).

Part of the [Vixeeny](../../VIXEENY_PLAN.md) workspace.
