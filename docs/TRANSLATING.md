# Adding or fixing a translation

Every user-facing string of Vixeeny lives in one file:
[`crates/vixeeny-common/src/i18n.rs`](../crates/vixeeny-common/src/i18n.rs). The settings window,
the overlay, the notifications and the daemon's tray menu all read it, so there is nothing to
translate anywhere else. The daemon uses this table without linking Slint or gettext, and the
compiler checks it for completeness.

French and English are shipped. English is the fallback for any other language.

## Fixing a text

Find the key (`grep` for the English sentence), edit the string for your language, keep any
`{placeholder}` exactly as it is — placeholders are replaced by the program (`{path}`, `{n}`…).
Run `cargo test -p vixeeny-common`: the `i18n` test fails if a text is empty or if two languages
do not use the same placeholders.

## Adding a language

1. Add the language to `enum Lang`, to `Lang::from_tag` (the primary tag, e.g. `"de"`) and to
   the `language` row in `crates/vixeeny-settings`. The installer has its own texts, in
   `packaging/windows/vixeeny.iss` (`[Languages]`, `[Messages]`, `[CustomMessages]`).
2. Give the `keys!` macro one more `$text:literal` and the `match` of `tr()` one more arm, then
   add the text of the new language to every key: an entry with a text missing does not compile.
3. Dates and numbers follow the language through the OS; no extra work.
4. Run `cargo test -p vixeeny-common -p vixeeny-ui` and look at the screenshots
   (`VIXEENY_SCREENSHOTS=/tmp/shots cargo test -p vixeeny-ui`) — long words can overflow a button.

## Adding a key (developers)

Add one line to `keys! { … }`: `Name => "English text", "Texte français";`. The enum, `Key::ALL`
and `tr()` are made from it.
