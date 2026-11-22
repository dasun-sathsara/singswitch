# Self-critique of the mocks (v1)

Produced by rendering `docs/mocks/connections.html` and `docs/mocks/screens.html` in Chrome at
1120x720 with the real Feixen Standard and Geist Standard files, then reading the pixels.

## What the render exposed that the plan text hid

1. **The route spine reads as a stepper, not a route.** Four equal-width stations with dots on a
   rule at the same y is exactly the wizard/stepper pattern. The values are also all mono bold at
   13, so `localhost`, `utun123`, `13.215.132.32:443` and `103.24.9.17` carry equal weight, and
   the eye reads four unrelated words. Fix: drop the rule-and-dot construction entirely. Make the
   spine a single line of text with arrows, left-aligned, at one size, with the *destination*
   emphasised: `localhost → utun123 → 13.215.132.32:443 → 103.24.9.17`, the last segment in ink
   and the earlier segments in ink2, all mono 12.5, no dots, no pins, no equal columns. Its
   second line, when running, carries the only coloured element: `Connected · 2h 14m`.

2. **The header does not establish hierarchy.** At 22/700 `home-vless` is smaller than the mock's
   own title bar, and the `13.215.132.32:443 · Reality · synced 2h ago` line at 13 sits close
   enough to the facts rows at 13 that the screen reads flat. Fix: name at 24/700, meta line at
   12.5 with the server in mono; facts at 13 but labels 12 `--muted` and values mono 12.5, so the
   two scales are different voices rather than two sizes of the same voice.

3. **The facts list is the best part and should own more of the screen.** It reads cleanly, one
   hairline per row, no boxes. But it starts 26px below the meta strip and the whole screen ends
   in dead ground. Fix: move the facts directly under the spine, and let a long empty stage be
   empty: no filler, no recent-activity list.

4. **The status strip is too quiet to do its job.** At 11.5 muted, `Connected · home-vless` is
   visually weaker than the rail's selected row, so the "state never leaves the screen" promise
   fails. Fix: 12px, `--ink2` for the values, dot at 6px, and the state word in `--live` only when
   running.

5. **The rail's selected row fights the accent.** A 2px accent bar at the left inset plus a 7%
   accent wash plus bold text is three signals for one state. Fix: keep the wash and the weight,
   drop the bar. Selection should be felt, not pointed at.

6. **Two identical greys.** `--recessed` (filter field) and `--hair` (rules) are close in value,
   so the filter field looks like a disabled control. Fix: filter field gets `--surface` with a
   hairline border, recessed is reserved for the segmented control's trough.

7. **Switches are unlabelled green blobs** in the settings mock, the exact "status colour used
   as decoration" failure. Fix: switch track uses `--accent` when on, `--hair` when off; green is
   only ever "the service is running".

8. **Doctor's glyphs are too small to carry state.** A 13px `✓` next to a 13px name reads as
   punctuation. Fix: 16px glyph in a 20px slot, and status words right-aligned in a fixed 72px
   column so they align down the page.

9. **The editor's context bar duplicates the rail and loses the route.** `Editing home-vless ·
   live · changes apply on save` at 13 in a 44px bar is fine, but nothing tells the user which
   profile is *running* while they edit a different one. Fix: the context bar carries
   `Editing <name>` on the left and `Running <name>` (or `Not connected`) on the right, in the
   same dot-and-words vocabulary as the status strip.

10. **Save isn't reachable when scrolled.** At 960x620 with the guided form at ~14 rows, the
    toolbar scrolls away. Fix: the editor's action row is part of the fixed context bar, which
    stays put; only the fields scroll.

11. **Logs: the detail panel sits outside the log frame** and looks like a second card. Fix: the
    detail panel is a bottom-docked region of the same framed container, separated by a hairline,
    not a second box.

12. **First run: three rows with mixed controls** (text, button, two buttons) at different
    alignments. Fix: every step row has the same right-aligned action slot, and only the next
    incomplete step has a filled button.

## Typography decisions confirmed by the render

- Feixen at 15 reads correctly at this density: rows of 52, 13 for secondary, 11.5 for labels.
  It is wider than SF Pro, so the rail needs 264px, not 240.
- Geist at 12.5 in mono columns is legible but the digits need tabular figures; the mock shows
  PID and duration columns misaligning by a pixel without `font-variant-numeric: tabular-nums`.
- 22px is enough for a screen title when the content below is dense. 24 is better for the
  Connections name because it is the screen's subject.

## Structural conclusions

- The rail + stage + strip holds up. It is the right shape for this product.
- The spine must become a line of text, not a diagram. That single change removes the only place
  where the design would have looked like a wizard.
- Density is right: 10 profiles fit before scrolling, and 8 facts fit without scrolling.
- The app needs one more thing the plan missed: a **profile state legend is unnecessary**, but a
  **"running" pin in the rail** is. When the selected profile is not the running one, the rail
  must show which row *is* running, even when that row is scrolled out of view; the status strip
  does that job, so no scroll-to behaviour is needed.
