# Review of 4e69c35 (grid controller keys, bypass, properties restyle)

| # | Point | Verdict at 4e69c35 | Evidence | Fixed in |
|---|-------|--------------------|----------|----------|
| 1 | `+` dropdown and right-click place on touching empty space | PARTIAL | `surface.rs` `pack_placed` put the cell at the exact cell (clamped) but never checked the spot was free: a taken cell or a span wider than the hole overlapped a neighbour; flowing cells listed earlier could take a requested spot; `pack_placed` duplicated ~80 lines of `pack`. Only "nodes control" and "scene" honour the place (`lanes.rs` `add_menu`), sequence/loop creation does not make a cell. | `pack` is now `pack_placed` with no places; a taken/too-small place slides to the nearest free spot; places are claimed before flow. 3 new tests |
| 2 | Enter saves + closes the controller editor | MET | `lanes.rs` `controller_editor`: `save \|\| key_pressed(Enter)` then `commit_controller_name` and close | - |
| 3 | Several buttons per graph | MET | `new_controller_key` (`<graph>#n`), every fn takes the key not the graph, persisted in `grid_controllers`, fired via `apply_scene(key)`, edited by key; switch second click applies the opposite (`active` / `!active`). Test `a_second_controller_for_one_graph_gets_a_unique_key` | - |
| 4 | Real bypass | PARTIAL | `paint.rs` skipped the bypassed op, and `out.sacn/artnet/fixture` already skip on bypass (`wire.rs:656,1890`). Gap: a bypassed `output.*` sink still kept its inputs' frames (`sink_inputs`), so it still shipped. Un-bypass was untested | `sink_inputs` ignores bypassed sinks; test covers bypass, un-bypass and a bypassed output |
| 5 | Gear/star/more | PARTIAL | Sizes equal (`CORNER_SIDE`), `Icon::Other` is the 3-dot ellipsis, black stroke, `panel_head` darker, `sharp_corner`: met. Gear was a stock `Button` with `min_size` (grows past its rect, glyph off-centre, rounded). Star lost its favourite state (`let _ = fav;`) | `components::square_icon_button` used by gear, star, more; star lights its square when favourite; test |
| 6 | Properties restyle | PARTIAL | Met: transparent panel + grid, solid header only, summary before mode, `detail` gone, `notice`, group titles. Missed: summary was one header row "From / To" with `←/→` prefixes (not From/Kind/Port + To/Kind/Port); key cells (`card_pad` margin) and label rows (frame stroke) had different heights; `segmented` still used for choices (9 sites); `"Layer"` literal bypassed `tr()` in `overlay.rs`, capitalisation done by a `section_title` hack that left timeline/lanes lowercase; `note` still grey; dead doc comment mangled in a test; rustfmt-style reflow noise in unrelated code | From/To blocks, `cell_height` rows, `choice` dropdowns, keys renamed `Layer`/`Effects Chain` (+ es), `note` = `notice` |

## Other findings

- Reformatting noise across `lanes.rs`/`components.rs` hid the real diff; avoid.
- `key_cell`/`payload_cell` and `label_row`/`row_label` painted the same look three times with different margins; now one `cell()` helper.
- `Theme::library_card_corner` still used by gallery/properties header only.
- Bypassed `fx.*` nodes stay passthrough in `effects.rs` chains but are skipped as ops in `paint`; left as is (not in the six points).

## Glass class set (Job 3)

`classes.rs`: `cell_height`, `glass_fill/veil/highlight/shade`, `key_shadow(_offset)`, `payload_shapes`, `key_shapes`, `group_shapes`, `payload_style`. `components.rs`: `key_cell`, `payload_cell`, `key_label`, `label_row`, `kv_wrap`, `card_section` (glass group), `notice`/`note`, `choice`. Blur is emulated with four layered shapes per cell (wash, frost over the lower half, 1px top highlight, 1px bottom shade); `glass_shapes_cost_next_to_nothing` builds 2000 cells under 16 ms in a debug build. Overlay grid phantom cells use `payload_shapes`. Screenshot: `docs/shots/properties-glass.png` (after only; a before shot needs a 4e69c35 build).
