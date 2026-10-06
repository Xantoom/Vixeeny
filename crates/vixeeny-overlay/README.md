# vixeeny-overlay

The zone editor of Print Screen, drawn natively: one borderless window per monitor whose content
is a DirectComposition visual tree. The frozen screen is uploaded once and never drawn again; the
veil, the zone's border and handles are visuals moved by the compositor; only the small pieces
that change (toolbar, style panel, magnifier, labels) are drawn, with Direct2D and DirectWrite.
Nothing presents frames the way a game does, so the screen keeps its refresh rate and the GPU
idles while the pointer rests.

Entry point: `Overlay` (same flow as the editor `Session` of `vixeeny-editor`).

Part of the [Vixeeny](../../VIXEENY_PLAN.md) workspace.
