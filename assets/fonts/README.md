# Bundled typefaces

Two Geist families under the SIL Open Font License 1.1, from
[vercel/geist-font](https://github.com/vercel/geist-font) v1.9.2.

- `Geist-*.ttf`: Geist Sans, proportional. Family `Geist`. Carries the interface.
- `GeistMono-*.ttf`: Geist Mono, fixed width. Family `Geist Mono`. Carries values, identifiers,
  logs and paths, where digits and addresses need to line up under each other.

Three weights each: Regular 400, Medium 500, Bold 700.

Medium is the emphasis weight. Geist at 700 is heavier than the Studio Feixen cut the interface
used to run, and at 13-15px it read as shouting; `fonts::EMPHASIS` points every title, heading,
profile name and log level at Medium instead. Bold is bundled but currently unused, kept so the
choice stays available.

`crates/singswitch-app/src/fonts.rs` embeds all six at compile time. If the assets are missing the
build still succeeds and text falls back to the platform UI face; the failure is logged rather
than fatal.
