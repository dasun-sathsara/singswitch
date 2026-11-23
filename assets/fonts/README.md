# Bundled typefaces

- `FeixenStandard-Regular.ttf`, `FeixenStandard-Bold.ttf` — Studio Feixen Standard. Proprietary licence; this
  repository is private. Do not redistribute the binaries.
- `GeistStandard-Regular.ttf`, `GeistStandard-Bold.ttf` — Vercel Geist Standard (SIL Open Font License 1.1).

Only Regular and Bold are bundled because only those two weights exist in the
supplied cuts. The app names these families and never requests 500 or 600,
which the text system would synthesise.

The binaries are embedded at compile time by `crates/singswitch-app/src/fonts.rs`.
If the assets are removed, the build still succeeds and text falls back to the
platform UI face; the failure is logged rather than fatal.
