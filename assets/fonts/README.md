# Bundled typefaces

Two Geist faces under the SIL Open Font License 1.1, from
[vercel/geist-font](https://github.com/vercel/geist-font) v1.9.2.

- `Geist-Regular.ttf`, `Geist-Bold.ttf`: Geist Sans, proportional. Family name `Geist`. Carries
  the interface.
- `GeistStandard-Regular.ttf`, `GeistStandard-Bold.ttf`: Geist Mono, fixed width. Family name
  `Geist Standard`. Carries values, identifiers, logs and paths, where digits and addresses need
  to line up under each other.

Only Regular and Bold are bundled for each, because those are the only weights the app asks for.
It never requests 500 or 600, which the text system would synthesise.

`crates/singswitch-app/src/fonts.rs` embeds all four at compile time. If the assets are missing
the build still succeeds and text falls back to the platform UI face; the failure is logged
rather than fatal.
