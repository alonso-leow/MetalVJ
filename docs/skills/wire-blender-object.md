# wire-blender-object — a Blender-made object driven by music

Verified against the code on 2026-11-23. Gaps are marked **GAP**.

## Pipeline

1. **Model in Blender, headless.** Settings ▸ Integrations installs Blender
   (flatpak `org.blender.Blender`); run `flatpak run org.blender.Blender -b file.blend --python export.py`.
   Export glTF 2.0 (`.glb`), one object, metres, Y-up, applied modifiers.
2. **Import.** AI tool `venue_asset_import(object, path, licence, scale?, at?, name?)`
   (`author/models/agent_tools.rs`) → `metal_geom::load_gltf` bakes every
   triangle (node transforms applied) into one `Mesh`, normalises the largest
   dimension to `scale` (default 1 m) and stores it as an `Object` with
   `form: Mesh` and its attribution. Licence is mandatory.
3. **Drive it.** The object itself has no audio params. Music reaches it
   through the model/venue floats that move it:
   - a model float `spec(name,min,max,default)` is 100 % drivable by the
     digest; `spec_held(…, drive)` is structural and damped;
   - the trailing signal block (`sig_strobe`, `sig_pan`, `sig_tilt`, `sig_a`,
     `sig_b`, `col_hue/sat/bri/contrast`, …) is appended to every model row
     (`SIGNAL_FLOATS_BLOCK` in `metal-schema`);
   - to move an imported object: bind a venue/object transform float
     (position, rotation, scale, material colour) to a digest column
     (kick, bass, …) with the node binding in the graph, same as any float.
4. **Texture route.** To show the mesh as a model, see the model-wiring notes
   (append-only `registry.rs`, regenerate `data/schema.json`).

## Naming convention for Blender (proposed)

Name an empty or custom property `mvj_<float>` (e.g. `mvj_scale_kick`) so the
exporter script can emit a binding list next to the `.glb`
(`<name>.bindings.json`: `{float, column, amount}`).

## GAPS

- **GAP:** `load_gltf` reads geometry only; glTF `extras`, shape keys and
  animation are ignored, so Blender custom properties do NOT arrive on their
  own. The `.bindings.json` sidecar above is not implemented yet.
- **GAP:** no MCP server for Blender is catalogued; headless CLI only.
- **GAP:** the exact object-transform → digest-column binding call was not
  traced; confirm in `call/venue.rs` before relying on step 3's last bullet.

See also: `docs/skills/logo-loop.md`.
