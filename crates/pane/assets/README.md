# Baked terminal art

## `startup-panes.txt`

26 frames, 30×14 glyphs, form-feed separated. A structure of four glass panes
rotating three quarters of a turn and settling square — the thing this program
is named for. The last frame is the resolved state and is what `/motion off`
shows.

Regenerate:

    python3 crates/pane/assets/startup-panes.gen.py 28    # writes /tmp/pane-art/seq/*.svg
    cd ~/projects/anythingToSomethingGreat && npm install && npm run build
    node - <<'JS'
    import { runJob } from './dist-node/node/api.js';
    for (let i = 0; i < 28; i++) {
      const id = String(i).padStart(3, '0');
      await runJob({ input: `/tmp/pane-art/seq/${id}.svg`,
        out: `/tmp/pane-art/ascii/${id}.html`, target: 'ascii-dom',
        preset: 'shade-mosaic', params: { asciiCell: 38 },
        frames: 1, fps: 12, seed: 7 });
    }
    JS

then crop each frame's `<script type="application/json">` text to rows 2..15,
columns 14..44, drop consecutive duplicates, and join with `\n\f\n`.

The glyphs are `░▒▓█` — the same ramp `tui/poster.rs` uses, which is why the
art and the transcript read as one thing. **No colour is baked in**: the file
is glyphs and the theme supplies the ink.
