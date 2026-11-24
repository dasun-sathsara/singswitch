# Bundled typefaces

Two Geist families under the SIL Open Font License 1.1, from
[vercel/geist-font](https://github.com/vercel/geist-font) v1.9.2.

- `Geist-*.ttf`: Geist Sans, proportional, family `Geist`. Used for the interface.
- `GeistMono-*.ttf`: Geist Mono, fixed width, family `Geist Mono`. Used for values, identifiers,
  logs, and paths, where digits and addresses must line up under each other.

Three weights each: Regular 400, Medium 500, Bold 700.

Medium is the emphasis weight. Geist at 700 is heavier than the Studio Feixen cut this interface
previously used, and at 13-15px it was too heavy, so `fonts::EMPHASIS` points every title, heading,
profile name, and log level at Medium instead. Bold is bundled but unused, kept for future use.

`crates/singswitch-app/src/fonts.rs` embeds all six at compile time. If the assets are missing
the build still succeeds, text falls back to the platform UI face, and the failure is logged.
