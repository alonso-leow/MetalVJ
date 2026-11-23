# logo-loop — autonomous logo and animated text (photocraft + effectcraft)

Needs Settings ▸ Integrations: photocraft and effectcraft installed
(`<data>/integrations/mcp.json` lists their MCP servers). Verified 2026-11-23.

## Loop

1. **Brief** → one line: name, mood, colours, font feel, static or animated.
2. **Draft (photocraft)** — transparent PNG:
   `photocraft-cli run --new '{"width":900,"height":300,"background":"transparent"}'
    --cmd type.create --params '{"text":"NAME","x":60,"y":200,"size":150,"color":"#ff3d00"}' --out logo.png`
   Param keys: `font`, `size`, `color`, `align`, `box`, `orientation`; list
   fonts with `type.fonts`, restyle with `type.setStyle`. Shapes: `shape.create`.
   Over MCP: `doc_new`, `command_run`, `doc_export`.
3. **Look** — open the PNG (`doc_render_preview` / `render_preview`, or Read the
   file). Judge against the brief: legibility, contrast, spacing, margins.
4. **Revise** — change ONE thing per round (size, colour, font, tracking);
   keep every round as `logo_v<N>.png`. Stop after 5 rounds or when it passes.
5. **Animate (effectcraft)** when asked:
   `effectcraft-cli --empty --save-as logo.ecproj run comp.new '{"name":"logo","width":960,"height":540,"duration":3,"background":"#000000"}'
    layer.newText '{"text":"NAME","size":120,"position":[480,300]}'
    layer.applyTextPreset '{"preset":"fadeUpCharacters"}'`
   Presets: typewriter, fadeUpCharacters, bounceInWords, trackingIn, scramble,
   blurIn, jitter, dropInLines. Check: `effectcraft-cli logo.ecproj render-frame --time 1.5 --out f.png`.
   Export: `render --out logo.webm --format webm --channels rgba` (or gif/png sequence).
6. **Deliver** the final files and the command log so it can be reproduced.

## Gotchas

- `effectcraft-cli run` without `--save-as` forgets the project at exit.
- `type.create` flattening warning is normal when saving to PNG.
- Each CLI call is a fresh session; chain commands in one `run`, or use `mcp`/`serve`.
