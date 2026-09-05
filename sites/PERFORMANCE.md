# Glass-render measurement — 2026-09-06

Browser: Chromium 152, ANGLE Metal, Apple M4 Pro, devicePixelRatio 1.
Measured on the local Vite server, with `?perf` enabling opt-in diagnostics.
GPU time uses EXT_disjoint_timer_query_webgl2. CPU time measures synchronous
engine.draw submission, not total browser CPU usage. First 31 CPU samples are
discarded; GPU samples include initial frames. No telemetry is transmitted.

| Configuration | CPU mean / p95 | GPU mean / p95 | Frame interval mean | Samples CPU / GPU |
| --- | --- | --- | --- | --- |
| Before, 1440×1000 viewport, render scale 1 | 0.271 / 0.400 ms | 1.523 / 2.730 ms | 33.338 ms | 600 / 600 |
| Final, 1440×1000 viewport, render scale 1.5 | 0.296 / 0.400 ms | 2.264 / 3.359 ms | 33.338 ms | 244 / 273 |
| Final, 390×844 viewport, render scale 1.5 | 0.285 / 0.400 ms | 1.213 / 2.049 ms | 33.338 ms | 213 / 242 |
| Final containment correction, 1440×1000, scale 1.5 | 0.277 / 0.400 ms | 2.174 / 3.506 ms | 33.423 ms | 198 / 227 |

The final desktop capture reports 54 renderer draw calls and 6,412 triangles
per frame. These are renderer counters, not an exhaustive GPU pipeline audit.
The higher sampling resolution increases GPU cost while improving edge quality.
The renderer meets its intentional approximately 30 fps cadence on this device.
CPU and GPU timings overlap; do not sum them as total frame time or interpret
them as system utilization percentages. Small viewport results are from this
Mac, not mobile hardware. No claim is made about battery cost or older GPUs.

Pause check: the frame counter stayed at 1,388 for 1.5 seconds after pausing.
390px viewport overflow check: no horizontal document overflow. Desktop before
and after screenshots were inspected. Tiny terminal paragraphs were replaced
with larger role names and simple terminal strokes; UI is rendered after the
transmission pass to avoid refracting text. A subsequent user correction restored
fixed 3D session positions and depth testing: foreground frame edges now occlude
the sessions, giving them an actual location within the house. Transparent glass
does not write depth. Some projection overlap is natural as the house rotates.
The house remains assembled throughout rotation.

The performance collector is only activated with `?perf` and retains at most
600 samples per series in memory. It is a diagnostic aid, not a benchmark suite.
