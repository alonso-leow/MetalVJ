# Node param cells

Every param row on a node card is one full-width **cell** (a slider, toggle,
dropdown, text, colour, vec or button), the dot on the card edge staying the
binding target.

- Geometry and hits live in `metal-graph` (`layout.rs`): `cell` primitives,
  `Hit::Cell`, `Hit::Toggle` (panel headers and the collapse chevron). Row
  sizes are consts scaled 1.4x from the Blender-like spec: `CELL_H` 22.4,
  `CELL_INSET` 11.2, `HEADER_H` 30.8 (22), `PORT_R` 7.0 (5), `PANEL_H` 25.2 (18).
- Colours and zoom thresholds are Theme tokens (`node_cell_*`). Painting and
  scrub math are `components::cell_*`, shared with Properties.
- Behaviour is `panels/nodes/cells.rs`: drag scrubs (Shift /10, Ctrl x10, Esc
  cancels, one write on release), hover arrows step 1/100 of the range,
  double-click types, right-click or Backspace resets (bound: unbinds),
  Ctrl+C/V copy/paste, Ctrl+wheel over a number steps it (elsewhere it zooms),
  enum opens a list, bool flips, colour opens the picker, vec scrubs one axis
  and writes the whole vector. Edits apply to every Ctrl+click-picked node
  with the same cell. Each gesture is one undo entry.
- Config keys become cells in `params::config_cells` (`venue.fixture` enums,
  `text.source` string/enums/bool); a `group` puts them in a collapsible panel.
  Open panels and the collapsed flag are stored on the node as `config.ui`
  (`open: [ids]`, `collapsed`), since layout needs them.
- Below zoom 0.6 cells are not editable (a press drags the node); below 0.5
  they paint as flat bars. Primitives outside the view are culled; the paint
  loop is the `nodes.cells` span in the F12 / `GET /perf` monitor.
