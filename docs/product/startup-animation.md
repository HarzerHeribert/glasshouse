# The startup animation

While a harness is starting, Glasshouse has nothing to show and several seconds
in which to show it. This is what fills them: a pitched-roof glasshouse turning
under a scan rule, with a seed suspended inside it, rendered as glyphs.

Everything needed to embed it is checked in under `assets/startup/`:

| file | what it is |
|---|---|
| `glasshouse.frames` | the asset. 96 frames, 68×20 glyphs, **89,101 bytes** (15,914 gzipped) |
| `glasshouse-scene.mjs` | the three.js scene the frames were rendered from |
| `build.sh` | regenerates `glasshouse.frames` from that scene |
| `pack.py` | strips the renderer's web player away and writes the frame bank |
| `play.py` | plays a frame bank in a terminal — the reference for the playback rules |

## Where the picture comes from

The subject is the product page's own, not a new one. `sites/DESIGN-RESEARCH.md`
describes the hero as "a complete pitched-roof glasshouse with separate walls,
roof panes, gables, and a seed inside it", and its motion as "a scan resolves
into an object, holds, then changes state in discrete steps". `sites/public/mark.svg`
is two offset outlined panes. So: the same building, the same scan, in glyphs.
Liquid glass does not survive a terminal, but *glazing bars, depth and a
measuring rule* do, and they are what the site is actually made of underneath
the refraction.

The renderer is [`anythingToSomethingGreat`](../../../anythingToSomethingGreat)
(a2sg), driven headlessly: it loads `glasshouse-scene.mjs` in Chromium, renders
each frame with three.js, and abstracts the result into a glyph mosaic whose
density ramp is *measured* from the font rather than guessed, with directional
glyphs substituted along detected contours. Its `ascii-dom` target emits an HTML
page whose payload is a JSON array of plain text frames; `pack.py` keeps that
array and throws the rest away.

Two decisions in the scene exist only because of the terminal, and both are
worth knowing before editing it:

- **The camera's aspect is the terminal's, not the framebuffer's.** a2sg samples
  a frame into cells of 28×48 pixels, but a terminal cell is about half as wide
  as it is tall. A frame drawn at the framebuffer's own aspect therefore arrives
  horizontally squeezed by ~14%. The scene frames to `cols/2 : rows` instead,
  which cancels it exactly.
- **The turn is not at a constant rate.** The gable and broadside views read
  cleanly at 20 rows; the three-quarter views between them are the busiest thing
  in the loop. The yaw carries a sinusoidal correction of a whole number of
  cycles per period, so the animation lingers on the two good angles and hurries
  through the two poor ones — while the angle *and its rate* still match at the
  seam. Measured: the wrap frame is the calmest transition in the loop (57 cells
  change, against a median of 145).

## The format

`glasshouse.frames` is plain UTF-8 text, designed to survive a code review and a
`git diff` rather than to be as small as possible.

```
# comments, until the first frame
%GHSTART 1
%grid 68 20            cols rows
%frames 96
%interval 80           milliseconds per frame
%loop cycle
%tier quiet  .:;
%tier soft   =+*-/|\
%tier bright #%@
@0
<up to `rows` lines, right-trimmed>
@1
...
```

Rules a loader must implement — all three are load-bearing:

1. **Lines are right-trimmed and padded back to `cols` on load.** An editor or a
   pre-commit hook that strips trailing whitespace cannot corrupt a frame, and
   the file is 33% smaller than the fixed-width form (89,101 bytes against
   133,507).
2. **A frame may have fewer than `rows` lines.** The missing ones are blank rows
   at the bottom. Read until the next `@` or the end of the file.
3. **Glyphs carry intensity, never colour.** Each of the eight glyphs that occur
   belongs to exactly one tier, and a tier is a *role*, not a colour: `quiet` is
   partial coverage where a line falls between cells, `soft` is the structure —
   the contour glyphs plus the mid ramp — and `bright` is the near frame and the
   seed. A theme maps those three onto its quiet, accent and secondary colours;
   nothing about the eight themes is baked into the asset.

The tiers are clean because the render's density ramp and its four contour
glyphs are **disjoint sets** — `customChars` in `build.sh` deliberately contains
no `-`, `/`, `|` or `\`. When they overlap (a2sg's stock `minimal` ramp contains
`-`), a cell whose contour rotates from horizontal to diagonal changes tier for
no reason the picture justifies. Measured on the same scene, one charset apart:
56 tier changes per frame with a2sg's stock `minimal` ramp, 47 with a disjoint
one. The shipped asset measures 49, having also gained a scan rule since.

Frequencies across the whole loop, for anyone tuning a theme: `-` 11373,
`\` 8246, `:` 4182, `|` 2703, `+` 2439, `%` 1670, `#` 1452, `*` 1294. About 364
of the 1360 cells carry ink in a typical frame.

## How it plays

**Frame rate.** `interval` is 80 ms, which is exactly five `DEFAULT_TICK`s
(`crates/glasshouse/src/tui/mod.rs`: 16 ms). The full loop is 96 × 80 ms =
**7.68 s**.

**Do not count ticks.** `Event::Tick` fires only when the event source has been
idle for the tick duration, so a busy session delivers fewer of them. Keep an
`Instant` from when the animation started and derive the frame:

```rust
let frame = (start.elapsed().as_millis() as usize / 80) % 96;
```

That is self-correcting, costs nothing when the frame has not changed, and makes
the animation independent of how often the shell happens to redraw.

**Looping.** `%loop cycle` — frame 95 is followed by frame 0, with no pause and
no reversal. The seam is exact by construction: over one period the building
turns 180° and it is symmetric about its own vertical axis, the seed turns 360°
and is symmetric about its long axis, and the scan pulse opens and closes inside
the period instead of wrapping.

**When the terminal is smaller than the art.** Crop from the centre; never
scale, and never reflow. The composition is built for this — ink never
reaches the frame edge, spanning columns 6–60 of 68 and rows 2–19 of 20 across
the whole loop, and a typical frame only spans columns 10–57. The scan rule's
ends and the plinth's corners are what a narrow terminal loses first. Below 40 columns or 12
rows there is no crop worth showing: print a line of text instead. `play.py`
implements exactly this, and it is the behaviour to copy.

**How it stops.** It is state, not a loop. The shell keeps "startup animation
running" as a field and draws the current frame during its normal render pass;
the moment the harness is ready it draws the session instead. There is no
fade-out, no wait for the loop to finish, and nothing to cancel — the animation
never owns the thread, so it cannot delay the thing it was covering. Frame 0 is
the calmest frame if a final still is ever wanted.

## Embedding it

```rust
const STARTUP_FRAMES: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/startup/glasshouse.frames"));
```

89 KB of `include_str!`. Parse it once into `Vec<Vec<&str>>` plus a
glyph→tier table; the parse is three `starts_with` checks per line.

## Rebuilding

```sh
# once: a2sg needs building, and it needs Chromium
cd ~/projects/anythingToSomethingGreat && npm install && npm run build

~/projects/glasshouse/assets/startup/build.sh          # ~7 s for 96 frames
~/projects/glasshouse/assets/startup/play.py           # watch the result
```

`build.sh` is deterministic: a2sg's `renderAt(time)` is a pure function of
`(frame, fps, seed, params, source)` with no wall clock anywhere, so the same
scene produces the same bytes.

**One trap, and it is silent.** a2sg lays its glyph atlas out as
`ceil(sqrt(n)) × ceil(n / cols)`, and its contour lookup only lands on the right
row when that grid comes out square. With nine custom ramp characters plus a
space plus the four contour glyphs — fourteen, a 4×4 grid — horizontals render
as `-` and verticals as `|`. With six ramp characters (eleven glyphs, a 4×3
grid) every horizontal renders as `/` and the picture turns into hatching, with
no warning from anything. Verified by rendering one frame per candidate charset.
**If you change `customChars`, keep it at nine characters**, or check a frame's
bottom edge before trusting the render.

## What this is not

- It is not a Rust change. This package produces the asset and this document;
  the player, the theme wiring and the "is the harness ready" state belong to
  whoever embeds it.
- The three-quarter views are busier than the gable and broadside views, and
  easing the turn manages that rather than fixing it. 20 rows is not enough
  resolution to draw two glazed walls seen through each other cleanly, and no
  parameter in the renderer changes that.
- Roughly 49 cells per frame change tier in place — the crawl any glyph
  rendering of a moving line has. On glass it reads as scintillation, which is
  why it was left alone rather than damped.
