# Bundled typefaces

- `FeixenStandard-Regular.ttf`, `FeixenStandard-Bold.ttf`: Studio Feixen Standard. Proprietary
  licence held for this private repository. Do not redistribute the binaries.
- `GeistStandard-Regular.ttf`, `GeistStandard-Bold.ttf`: Vercel Geist Standard, SIL Open Font
  License 1.1.

Only Regular and Bold are bundled because only those two weights exist in the supplied cuts. The
app never requests 500 or 600, which the text system would synthesise.

`crates/singswitch-app/src/fonts.rs` embeds the binaries at compile time. If the assets are
missing the build still succeeds and text falls back to the platform UI face; the failure is
logged rather than fatal.
