# Product direction and motion research

Updated 2026-09-06. This is website-specific research, not a change to the
Glasshouse implementation strategy or capability map.

## Positioning

Glasshouse is the orchestration layer for visible native coding-agent sessions.
Pane is the first-party harness, usable independently or as a Glasshouse session.
Keep separate product pages and separate acquisition paths. The shared homepage
explains the relationship before inviting the reader into either product.

Avoid claims of universal provider support or demonstrated cost superiority.
The current capability map does not establish either for Pane. Its runtime,
event handling, guarded continuations, and direct completion are explicitly
labeled as in development or proposed. The most striking features are maintained
in `src/products.js`, with six entries per product.

Evidence: `../README.md` (What it does today) and
`../docs/product/capability-map.md` (Phase 61). Existing working foundations and
future design are intentionally distinguished in visitor-facing copy.

## Visual transfer

Reference: https://www.marathonthegame.com/

The useful transfer is the contrast between a detailed recognizable specimen
and its machine-readable representation: a scan resolves into an object, holds,
then changes state in discrete steps. It is not arbitrary glitching or labels
pretending to be telemetry. No claim is made about Bungie's internal toolchain.

Our subjects are a maple samara and a chambered shell, not frogs or moths. The
seed suggests movement and distribution; the shell suggests retained structure.
These are visual metaphors, not scientific explanations of the products.

The hero uses a complete pitched-roof glasshouse with separate walls, roof
panes, gables, and a seed inside it. Glass recurs around specimen sequences on
the homepage and product pages. The reading surface remains open and unboxed.
No decorative figure labels, crosshairs, specimen IDs, random counters, frosted
cards, or backdrop blur.

## Animation-library shortlist

This is a current suitability comparison, not an unsupported popularity ranking.

| Library | Mechanism and strength | Decision for this site |
| --- | --- | --- |
| Three.js | GPU scene rendering; physical transmission, IOR, thickness, dispersion, and custom shaders | Retain for real glass geometry and clear specimen refraction |
| GSAP + ScrollTrigger | Timelines over object properties and DOM, stepped easing, scroll-triggered sequences | Add for specimen-state timing and restrained feature reveals |
| Motion | DOM/SVG animation and scroll-linked effects using native ScrollTimeline where possible | Good lightweight alternative for an interface-led page; overlaps GSAP here |
| Anime.js | Modular animation engine with SVG drawing, morphing, and motion paths | Strong alternative for a predominantly vector identity; not needed alongside GSAP |
| Theatre.js | Visually authored animation sequences and keyframes over object properties | Consider if the 3D scene becomes an art-directed film; unnecessary editor/state layer today |
| Rive | Authored vector assets and interactive state-machine runtime | Consider for a reusable interactive mascot; would require a separately authored asset pipeline |

Primary sources:

- https://threejs.org/docs/pages/MeshPhysicalMaterial.html
- https://gsap.com/docs/v3/Plugins/ScrollTrigger/
- https://gsap.com/docs/v3/GSAP/gsap.matchMedia()/
- https://motion.dev/docs/scroll
- https://motion.dev/docs/animate
- https://animejs.com/documentation/svg/
- https://www.theatrejs.com/docs/latest/api/core
- https://rive.app/docs/runtimes/web/web-js

## Optical changes

The first shader merged overlapping pane distance fields, producing abrupt
normal changes at their intersections. It also exaggerated RGB separation.
The hero now uses actual separate beveled glass meshes with zero roughness,
full transmission, thickness, and restrained dispersion. Its edges respond to
the underlying scene rather than a merged 2D silhouette. This remains a
real-time approximation, not offline optical ray tracing.

The specimen shader uses one continuous pane boundary, central-difference
normals, screen-space derivative antialiasing, and smoothly bounded edge
displacement. It samples sharp artwork without a blur pass. GSAP sequences
move between a detailed specimen and a quantized terminal raster, with a quiet
hold between transitions. No flashing or text scrambling.

## Performance and motion

The house is six clear panes joined edge to edge: two side walls, two pentagonal
ends, and two roof panes. There are no frames, mullions, steps, foundation,
floor, ridge vent, or hanging-light supports. A warm point light floats inside.
A separate projected light layer builds
GLASS / HOUSE lettering from dots and integrates its aperture texture radially
to produce a corona. It occasionally slips rows and quantizes the rays under a
slow envelope. This is a stylized light-shaft approximation, not physically
traced caustics. No glass roughness or frosting was introduced.

Initialize scenes near the viewport, cap device pixel ratio, render at roughly
30 fps, pause offscreen and when the tab is hidden. Respect reduced-motion at
startup and on changes. Keep a global motion pause control. Preserve readable
content and static fallback visuals if graphics initialization fails.

## Original artwork

Built-in image generation produced `public/specimens.png` (1536 × 1024).
Prompt: "Two scientifically recognizable isolated natural specimens: a paired
maple samara on the left and a cutaway chambered nautilus shell on the right;
museum specimen macro photography, cool monochrome silver-white on near-black,
sharp veins and shell partitions, generous margins, no type, labels, borders,
frogs, or moths." The generated result was inspected and copied into the site.
It is illustrative generated artwork, not a source photograph or scientific record.
