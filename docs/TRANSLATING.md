# Adding or fixing a translation

Every user-facing string of Vixeeny lives in one file:
[`crates/vixeeny-common/src/i18n.rs`](../crates/vixeeny-common/src/i18n.rs). The settings window,
the overlay, the notifications and the daemon's tray menu all read it, so there is nothing to
translate anywhere else. (The plan mentions `.po` files and `@tr()`: the project uses this Rust
table instead, which the daemon can use without linking Slint or gettext, and which the compiler
checks for completeness.)

French and English are shipped. English is the fallback for any other language.

## Fixing a text

Find the key (`grep` for the English sentence), edit the string for your language, keep any
`{placeholder}` exactly as it is — placeholders are replaced by the program (`{path}`, `{n}`…).
Run `cargo test -p vixeeny-common`: the `i18n` test fails if a text is empty or if two languages
do not use the same placeholders.

## Adding a language

1. Add the language to `enum Lang`, to `Lang::from_tag` (the primary tag, e.g. `"de"`) and, in
   the settings and the assistant, to the language lists (`LANGUAGES` in
   `crates/vixeeny-ui/src/wizard_panel.rs`, the `language` row in `crates/vixeeny-settings`).
2. The compiler then points at every `(Key, Lang)` arm of `tr()` that is missing: add one per key.
   Nothing can be forgotten, the `match` is exhaustive.
3. Dates and numbers follow the language through the OS; no extra work.
4. Run `cargo test -p vixeeny-common -p vixeeny-ui` and look at the screenshots
   (`VIXEENY_SCREENSHOTS=/tmp/shots cargo test -p vixeeny-ui`) — long words can overflow a button.

## Adding a key (developers)

Add the variant to `enum Key` with a doc comment listing its placeholders, one arm per language in
`tr()`, and the variant to `Key::ALL`. The test fails when `Key::ALL` is out of date.
