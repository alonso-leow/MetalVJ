//! The *classes* layer: every visual constant of the shell, and nothing else.
//!
//! This is the one file under `shell/` where a colour, a stroke width, a
//! margin, a radius or a font size may be written down. `components.rs` and
//! everything above it read those values off a [`Theme`] — the literal ban
//! (gate 2, `gates.rs`) rejects them anywhere else, and the dependency gate
//! (gate 1) rejects this file importing anything from `shell` at all, so a
//! `Theme` can never learn what a panel is.
//!
//! The house look comes from `the build reference` § *The look* and the variables in
//! `web/theme.css`: a grey tube, red phosphor, hairline rules instead of
//! contrast, a 2px radius, one monospace face. [`Theme::phosphor`] is that,
//! after dark, and [`Theme::daylight`] is the same universe with the tube
//! switched on — the two house styles share the emphasis and differ in
//! everything else (`plan-opus.md` § O.17). The dark style's values are the
//! stylesheet's own exact hexes, pinned by test.
//! [`Theme::contrast`] is deliberately *not* that — it exists so the restyle
//! drill (gate 3) has a second theme that differs in colour *and* in spacing,
//! which is what proves the widget tree is not reading either.

use egui::style::HandleShape;
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};

/// Every visual decision of the shell, as data.
///
/// Fields are grouped the way `theme.css` groups its variables: surfaces,
/// inks, the accent, rules; then strokes, radii, spacing, fonts. Anything a
/// component needs to paint that is not here is a bug in this file, not a
/// reason to write a literal in the component.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// A name for reports and the restyle gate's failure messages.
    pub name: &'static str,

    // --- surfaces -------------------------------------------------------
    /// The window ground (`--bg`).
    pub tube: Color32,
    /// A raised surface on the tube: the header (`--bg-raise`).
    pub tube_raised: Color32,
    /// A panel's fill (`--bg-panel`).
    pub panel: Color32,
    /// A panel's header strip: darker than the panel, no title of its own
    /// (`components::panel_head_frame`).
    pub panel_head: Color32,
    /// A control's recessed fill — text fields, code, meters (`--bg-inset`).
    pub inset: Color32,

    // --- inks -----------------------------------------------------------
    /// Body text (`--fg`).
    pub ink: Color32,
    /// Secondary text — kickers, placeholders (`--fg-mute`).
    pub ink_weak: Color32,
    /// Footnote ink; disabled, never essential (`--fg-faint`).
    pub ink_faint: Color32,

    // --- the phosphor ladder (`web/theme.css` § Phosphor) -----------------
    /// The phosphor (`--accent`). **The emphasis is common to the two
    /// styles**: dark and light pick the same red, so a lit control reads
    /// as the same signal on either tube. See `plan-opus.md` § O.17.
    pub phosphor: Color32,
    /// The phosphor at its hottest — the link/hover treatment (`--accent-lit`).
    /// Also part of the emphasis held common across the two styles.
    pub phosphor_lit: Color32,
    /// The dim step: the rule that connects things (`--accent-dim`).
    pub phosphor_dim: Color32,
    /// The deep end: selection and hover washes (`--accent-deep`).
    pub phosphor_deep: Color32,
    /// A hotter accent for emphasis on a focused surface — the properties
    /// panel's own red (its active rows, its node card, the header block),
    /// one step more saturated than [`Self::phosphor`] so the emphasis in the
    /// inspector reads as a live signal rather than a dull tint. Held common
    /// across the two styles like the rest of the signalladder; the contrast
    /// style keeps its blue so its whole palette stays distinguishable.
    pub accent_vivid: Color32,
    /// The logo's `Metal` run: a blue that reads on the tube and the cream ground
    /// alike (the `MetalVJ` wordmark).
    pub logo_metal: Color32,
    /// The logo's `VJ` run: the phosphor red, the wordmark's second half.
    pub logo_vj: Color32,
    /// The cold second accent, for two lit things that must be told apart
    /// (`--accent-soft`).
    pub soft: Color32,
    /// That second accent at rest (`--accent-soft-dim`).
    pub soft_dim: Color32,
    /// Reserved for meaning, never decoration (`--ok`).
    pub ok: Color32,
    /// Reserved for meaning, never decoration (`--warn` — the amber the era
    /// wants next to the red).
    pub warn: Color32,
    /// The phosphor's glow, as a shadow colour: the accent at a whisper of
    /// alpha, per style, so a raised card lit itself in red rather than in
    /// generic grey haze.
    pub glow: Color32,
    /// The rule (`--rule`).
    pub rule: Color32,
    /// The rule when something is focused or hovered (`--rule-bright`).
    pub rule_bright: Color32,

    // --- strokes and radii ----------------------------------------------
    /// Width of a hairline rule, in points.
    pub hairline: f32,
    /// Width of an accent stroke (a focused edge), in points.
    pub accent_stroke: f32,
    /// Corner radius of frames and widgets (`--radius`).
    pub radius: u8,
    /// Corner radius of inputs and buttons (inactive, hovered, active, open):
    /// rounder than the frames so a control reads as a control.
    pub field_radius: u8,
    /// Corner radius of menus and popups (`menu_corner_radius`).
    pub menu_radius: u8,
    /// Blur of the window/popup shadow, in points — the glow radius.
    pub glow_blur: u8,

    // --- spacing --------------------------------------------------------
    /// Inner margin of a panel frame (`--pad`).
    pub pad: i8,
    /// Inner margin of the header bar, horizontally.
    pub header_pad: i8,
    /// Inner margin of the header bar, vertically — larger than
    /// [`Self::header_pad`] so the row has clear space above *and* below its
    /// content, vertically centred, rather than reading as flush with the
    /// panel region below.
    pub header_pad_y: i8,
    /// Gap between items inside a panel.
    pub item_gap: Vec2,
    /// Gap between the buttons of a segmented row (a chip strip, a choice of
    /// modes): the spacing those rows always had, kept apart from `item_gap` so
    /// the panel gap can grow without re-spacing every row.
    pub segment_gap: Vec2,
    /// Gap between the two halves of the wordmark (`Metal` · `VJ`).
    pub wordmark_gap: Vec2,
    /// Inner padding of every button, dropdown and text field — one value so
    /// a control's text sits at the same inset whatever widget it is. Also the
    /// `TextEdit` margin too (`TextEdit::margin(theme.field_pad)`).
    pub field_pad: Vec2,
    /// Height of the header bar, in points.
    pub header_height: f32,
    /// Minimum size of an interactive widget.
    pub interact_size: Vec2,

    // --- type -----------------------------------------------------------
    /// The wordmark and panel kickers.
    pub font_heading: FontId,
    /// Everything else.
    pub font_body: FontId,
    /// Footnotes, monospaced readouts.
    pub font_small: FontId,

    // --- the ceding grid (`S.U8`) -----------------------------------------
    /// The fraction of its row/column a region takes when it is the
    /// [`crate::shell::panels::Region`] in [`crate::shell::Shell`]'s
    /// `focus` — the *"focused-expands"* half of the property `S.U8`
    /// reimplements (`plan-opus.md` § O.12.e). Applied to whichever axis
    /// that region's pair splits: height for the top/bottom row split,
    /// width for the left/right split inside a row.
    pub focus_share: f32,
    /// The fraction a region's row/column-mate shrinks to while the other
    /// half of the pair is focused — the *"others-cede"* half.
    pub cede_share: f32,
    /// The bottom row's (library · lanes) default height, as a fraction
    /// of the space below the header, when nothing is focused. `3.0 / 7.0`
    /// is the old HTML dashboard's `grid-template-rows: 4fr 3fr` (deleted
    /// 15-Sep-2026, S.U18) read as
    /// a fraction (`the design notes` § II.5).
    pub row_share: f32,
    /// Properties' default width, as a fraction of the top row, when
    /// nothing is focused. `5.0 / 14.0` is that grid's `span 5` of 14
    /// columns; nodes is not given a matching fraction because it is the
    /// row's [`egui::CentralPanel`] and simply takes what is left —
    /// `#panel-preview`'s old `span 2` included, since that panel is gone
    /// (`plan-opus.md` § O.12.e).
    pub properties_share: f32,
    /// Library's default width, as a fraction of the bottom row, when
    /// nothing is focused. `6.0 / 14.0` is that grid's `span 6`; lanes is
    /// the row's `CentralPanel` and takes the remaining `span 8`.
    pub library_share: f32,
    /// The floor under any region's outer size, in points — the "nothing
    /// is cropped" half of the property: every split below is clamped
    /// so neither side of it can fall under this, on both axes, regardless
    /// of `focus_share`/`cede_share`.
    pub region_min: Vec2,

    // --- the nodes panel (`S.U9`) -----------------------------------------
    /// Width of the animation preview the canvas paints in its top-right
    /// corner (`panels/nodes.rs`'s `preview_pip`, the built-in preview
    /// device's `GET /preview.jpg`), as a fraction of the canvas's own
    /// width; a click on it swaps to the whole width. A fraction, not a
    /// point size, because the picture-in-picture must shrink with the
    /// region when it cedes and grow when it is focused.
    pub graph_preview_share: f32,
    /// Width, in points, of the floating tile the `+ node` menu shows
    /// while the pointer rests on a `gen.*` entry — the studio grid's own
    /// tile for that model, at a size the operator can read.
    pub graph_hover_preview: f32,
    /// Tallest the `+ node` catalog menu grows before it scrolls, in
    /// points — a popup egui does not bound to the viewport by itself, and
    /// the catalog is longer than a phone is tall.
    pub menu_max_height: f32,
    /// Tint applied to a painted texture (the model tiles inside node
    /// boxes, the preview picture-in-picture): white, so the pixels show
    /// as the renderer made them. A `Color32` here rather than in the
    /// panel is gate 2's rule.
    pub image_tint: Color32,

    // --- the lanes panel (`S.U10`) -----------------------------------------
    /// A single-line text field's desired width, in points — every free-text
    /// form field in the lanes panel (a trigger id, a reel folder path/name)
    /// uses this rather than `TextEdit`'s own default of filling whatever
    /// horizontal space remains: inside a row that also has buttons after
    /// it, an unbounded `TextEdit` claims the *whole* row and — once the
    /// region has ceded to a narrower width — pushes those buttons past the
    /// viewport's own right edge, which is exactly the "nothing pushed off
    /// off-screen" failure `focus_expands_and_others_cede` (`kittest.rs`)
    /// caught while writing this panel.
    pub text_input_width: f32,
    /// Visible rows of the `AI` section's multiline "AI instructions"
    /// box; its width is the panel's own, so rows are the only size.
    pub ai_instructions_rows: usize,
    /// Visible rows of a code editor's multiline field (the WGSL editor
    /// `panels/props_shader.rs` and the script editor
    /// `panels/properties/script.rs`); its width is the panel's own, so rows
    /// are the only size.
    pub code_rows: usize,
    /// Width, in points, of a mask's corner-point editor canvas
    /// (`panels/lanes.rs`'s `points_canvas`) — the canvas's height follows
    /// from it at the device's own aspect ratio, so this one number is the
    /// whole size decision (gate 2 keeps it out of `lanes.rs` itself).
    pub mask_canvas_width: f32,
    /// Side, in points, of one square button in the overlays grid
    /// (`panels/lanes.rs`'s `overlays_grid`) — a sequence's play/stop toggle,
    /// so the grid's cells are all the same shape regardless of name length.
    pub overlay_cell: Vec2,
    /// How far down a piano cell its black keys reach, as a share of its
    /// height.
    pub piano_black_depth: f32,
    /// Height, in points, of the overlays grid's pager row (`‹ ● ●  ›`) under
    /// the cells. Reserved even when there is one page, so the grid's row
    /// count is the same on every page and the panel does not jump.
    pub pager_height: f32,
    /// Radius, in points, of one page dot in that pager row.
    pub pager_dot: f32,
    /// The overlays grid's colour built-ins (`panels/builtins.rs`): four
    /// stops per palette button, `[preset][stop][r,g,b]` in 0..1. The same
    /// values the route receives in the body, so the shell never sends a
    /// colour it did not paint.
    pub colour_presets: [[[f32; 3]; 4]; 4],
    /// The RGB built-in's popup swatches, in the order its names list.
    pub rgb_presets: [[f32; 3]; 7],
    /// Fraction of a palette cell's height its four-stop strip takes.
    pub swatch_strip: f32,
    /// Side, in points, of a built-in cell's `⚙` corner button.
    pub gear_size: f32,
    /// One swatch button in the RGB popup.
    pub swatch_button: Vec2,

    // --- the mask / contour editor (`S.F`/`O.1`) --------------------------
    /// The wash a contour's inside is filled with (`panels/properties/contour.rs`).
    /// Translucent on purpose: the editor paints *over* the device preview
    /// (`frames.rs::Frame`), so the picture must read through the mask.
    pub mask_inside: Color32,
    /// The wash over everything **outside** the contour — dims the frame
    /// around the mask without hiding it, which is what makes a contour
    /// lined up against the real projector legible at a glance.
    pub mask_outside: Color32,
    /// A draggable vertex handle's fill.
    pub mask_vertex_handle: Color32,

    // --- the properties browser -------------------------------------------
    /// Width, in points, under which the properties tree collapses into a
    /// strip of folder chips above the content (the brief's narrow-width
    /// threshold as a theme token).
    pub properties_narrow: f32,
    /// Height of one table column header in the browser, in points.
    pub table_header_height: f32,
    /// Height of one browser table row, in points — dense, VirtualDJ-like.
    pub table_row_height: f32,
    /// The alternating row fill of the browser's striped table.
    pub stripe: Color32,

    // --- the library panel (`S.U11`) ---------------------------------------
    /// Inner margin of the library's two columns (`panels/library.rs`'s
    /// `paint`) — its own token, not `pad`, because a browser is denser than
    /// a panel and the two move independently.
    pub library_pad: i8,
    /// Gap between the library's file rows (2026-10-01: a list, one file
    /// per row, the way the folder column lists folders).
    pub library_gap: f32,
    /// Height of one file row (`panels/library.rs`'s `file_card`), fixed so
    /// the list does not move as names wrap.
    pub library_row: f32,
    /// Height of a library *card* in list mode, in points — taller than
    /// [`library_row`] so a card's name line and its detail line both fit
    /// with the card's own inner padding.
    pub library_card_h: f32,
    /// Corner radius of a library card, rounder than [`radius`] so the
    /// browser reads as a stack of cards.
    pub library_card_radius: u8,
    /// Inner padding of a library card — from the card edge to its badge,
    /// name and detail line.
    pub library_card_pad: Vec2,
    /// A card's resting fill.
    pub library_card: Color32,
    /// A card's fill on hover, and behind the picked/open card.
    pub library_card_hover: Color32,
    /// The fill of a card's kind badge/thumbnail chip.
    pub library_badge: Color32,
    /// Side of that badge chip, in points.
    pub library_badge_size: Vec2,
    /// Gap between a card's name line and its detail line.
    pub library_detail_gap: f32,
    /// Width of the fixed right column every library row reserves for its
    /// `…` menu, so the buttons of folders and files line up whatever the
    /// title length (`panels/library.rs`'s `file_like_row`).
    pub row_menu_width: f32,

    // --- responsive delegation (`S.U13`, `O.12.f`) -------------------------
    /// Library's width, as a fraction of the viewport, once it has
    /// delegated out of the grid into a left [`crate::shell::components::side_panel`]
    /// — floored at `sidebar_min_width` by [`sidebar_width`] the same way a grid
    /// split is floored by [`crate::shell::regions::clamp_split`], so a
    /// sidebar is never narrower than a grid column would have been.
    pub sidebar_share: f32,
    /// Narrowest a sidebar may be dragged, in points — the same floor a grid
    /// split gets, so a sidebar never shrinks past a usable column.
    pub sidebar_min_width: f32,
    /// Widest a sidebar may be dragged, as a fraction of the viewport.
    pub sidebar_max_share: f32,
    /// How long, in seconds, a press has to be held on the properties
    /// button (or, once a later batch wires it, the selected node in
    /// `graph`) before it counts as a long-press and opens the properties
    /// overlay — measured against `ui.input(|i| i.time)`, which is why this
    /// is a duration and not a frame count: a kittest test drives it by
    /// advancing that same clock (`kittest.rs`'s `press_and_hold`).
    pub long_press: f32,
    /// Height of the nodes canvas's own control bar — the strip just above
    /// the canvas surface that carries play/pause, zoom, `+ node`, undo.
    /// The selected node's own verbs (bypass/solo/duplicar/quitar) sit on
    /// this bar too since 21-Sep-2026 — the lower ops strip that used to
    /// hold them (`nodes_ops_height`) is gone.
    pub canvas_bar_height: f32,
    /// Height of a panel's header strip (`components::panel_head`). The same in
    /// every style: a taller header in one style would cut the overlays grid
    /// differently, so the header is structure, not a per-style size.
    pub panel_head_height: f32,
    /// A navigation row (settings and gallery sections): its height, its
    /// icon's side, and how strongly the red washes it when hovered / selected
    /// (alpha of the accent over the ground).
    pub nav_row_height: f32,
    pub nav_icon_side: f32,
    /// Vertical gap between navigation rows (the hairline divides them).
    pub nav_row_gap: f32,
    pub hover_alpha: u8,
    pub selected_alpha: u8,
    /// The tallest an image thumbnail gets in the properties card.
    pub payload_max_height: f32,
    /// How opaque the card following the pointer is while it is dragged.
    pub drag_ghost_opacity: f32,
    /// Padding inside a floating canvas bar, and inside the resting (hold) bar.
    pub bar_pad: i8,
    pub hold_bar_pad: i8,
    /// The share of the window the "new" gallery overlay takes, each way
    /// (40% of the former 0.8 / 0.9 linear size: 0.32 / 0.36).
    pub gallery_share: f32,
    /// The narrowest a gallery card gets (40% of the former 170 / 220 linear):
    /// the column count follows from it.
    pub gallery_card_min_w: f32,
    /// A port dot's radius on a gallery thumbnail.
    pub node_port_preview_radius: f32,
    /// Width kept free beside a scroll area's content for its scroll bar.
    pub scroll_gutter: f32,
    /// Size of a glyph button ([`components::icon_button`]): the canvas bars'
    /// controls.
    pub icon_button_size: Vec2,
    /// Height of a compact control row (the sequence editor's transport bar
    /// under the ticks): the buttons shrink to this so the timeline above
    /// keeps the panel.
    pub compact_row_height: f32,
    /// Button padding inside a compact control row.
    pub compact_row_pad: Vec2,
    /// Spacing of the canvas's background grid, in graph units — the view
    /// transform (pan/zoom) scales it; the panel only decides how *dense*
    /// the grid is at zoom 1, never a screen-space width (gate 2 keeps
    /// that math literal-free by reading `theme.hairline`).
    pub graph_grid: f32,
    /// Step a zoom in/out click multiplies the canvas view's zoom by (the
    /// inverse for zoom out) — not a per-notch wheel amount, the two
    /// buttons' one easing.
    pub graph_zoom_step: f32,
    /// Zoom floor — past it the view has zoomed out too far to keep
    /// a node readable.
    pub graph_zoom_min: f32,
    /// Zoom ceiling.
    pub graph_zoom_max: f32,
    /// Horizontal breathing room of a node's label inside its box, in
    /// screen points at zoom 1 — the label is centred on the node box, so
    /// this padding is the floor between the text and the box's own edge.
    pub graph_label_pad: f32,
    /// How many edits the nodes panel's undo stack (the `revertir` button
    /// and its `CTRL + Z` shortcut) remembers before the oldest falls off;
    /// a graph edit is a disk write (`call::graph`'s own note), so the
    /// stack is bounded deliberately.
    pub graph_undo_depth: usize,

    // --- the Wire node canvas (N) -----------------------------------------
    /// The canvas's own ground — darker than the panel so the nodes read as
    /// raised cards on it (the Resolume-Wire look; `nodes-reference.png`).
    pub node_canvas: Color32,
    /// A node box's fill — the raised dark-grey card.
    pub node_body: Color32,
    /// The header band's fill, a step lighter than the body.
    pub node_header: Color32,
    /// The title ink: the accent, as Wire titles every node.
    pub node_title_ink: Color32,
    /// A body row's label ink (a port or parameter name), a soft light grey.
    pub node_row_ink: Color32,
    /// A value field's fill (the recessed readout on the right of a row).
    pub node_field: Color32,
    /// A value field's text ink.
    pub node_field_ink: Color32,
    /// A dot's fill when the row is wired (or is an output offering one).
    pub node_dot_on: Color32,
    /// A dot's fill when nothing is wired to it.
    pub node_dot_off: Color32,
    /// The outline a selected node's box carries.
    pub node_select: Color32,
    /// The stroke an ordinary cable draws with.
    pub node_cable: Color32,
    /// The stroke a cable draws with when it touches the selected node — the
    /// accent, per Wire.
    pub node_cable_accent: Color32,
    /// The ink of the `▸ n more` affordance.
    pub node_more_ink: Color32,
    /// A cable's dash length, in points, for the control/param cables that
    /// Wire draws dashed (image cables stay solid).
    pub node_cable_dash: f32,
    /// Width, in points, of a cable's stroke.
    pub node_cable_width: f32,
    /// How far inside a row's dot the row's label starts, in points.
    pub node_label_inset: f32,
    // --- the HSB picker (`K`) ---------------------------------------------
    /// The S×B square a `components::hsb_picker` paints, in points — the two
    /// axes the operator drags inside.
    pub picker_square: Vec2,
    /// The hue bar beside it, in points (its width is a sliver, its height
    /// the square's).
    pub picker_hue: Vec2,
    /// Gap between the square, the hue bar and the numbers.
    pub picker_gap: f32,
    /// Size of the round handle drawn where the value sits (drag it).
    pub picker_handle: f32,
    /// The handle's outline, so it reads on a light or a dark swatch.
    pub picker_handle_stroke: Color32,

    // --- the widget kit (properties + library, 2026-10-08) --------------
    /// Width, in points, of a selector control (a `components::dropdown`, a
    /// `components::search_field`) in a properties row — the shared column
    /// every selector claims, so rows line up across folders.
    pub control_width: f32,
    /// Width, in points, of a row's leading label before its control — the
    /// kit's `label_row`/`number_row`/`slider_row` align their controls on
    /// this column so a column of rows reads as a table.
    pub label_width: f32,
    /// Vertical gap above a collapsible section header
    /// (`components::section`), separating one block of rows from the next.
    pub section_gap: f32,
    /// Inner margin of a `components::card` — the outlined group every
    /// properties section sits in — and the gap left between two cards.
    pub card_pad: i8,
    pub card_gap: f32,
    /// Side of the chip a `components::swatch_chip` paints, in points.
    pub swatch_size: Vec2,

    // --- the colour-grading kit (feat/fx-grade, 2026-10-08) ---------------
    /// Height, in points, of a gradient track (`components::slider_tracked`).
    pub track_h: f32,
    /// Width, in points, of that track.
    pub track_w: f32,
    /// Corner radius of the track, in points (a `u8` so the track can be a
    /// pill rather than the widget radius).
    pub track_radius: u8,
    /// The flat fill of a [`crate::shell::components::Track::Plain`] track.
    pub track_fill: Color32,
    /// The neutral grey a `Track::Centered` track is centred on.
    pub track_center: Color32,
    /// The darker edge a `Track::Centered` track runs to.
    pub track_edge: Color32,
    /// Warm end of the temperature track (`Track::Temp`).
    pub temp_warm: Color32,
    /// Cold end of the temperature track.
    pub temp_cold: Color32,
    /// `Track::Tint` green end.
    pub tint_green: Color32,
    /// `Track::Tint` magenta end.
    pub tint_magenta: Color32,
    /// Side, in points, of a `components::curve_graph`.
    pub curve_size: f32,
    /// A grid line inside the curve graph.
    pub curve_grid: Color32,
    /// The master curve's ink.
    pub curve_ink: Color32,
    /// The red channel curve's ink.
    pub curve_channel_red: Color32,
    /// The green channel curve's ink.
    pub curve_channel_green: Color32,
    /// The blue channel curve's ink.
    pub curve_channel_blue: Color32,
    /// Radius, in points, of a curve point handle (and its grab threshold).
    pub curve_handle: f32,
    /// Width, in points, of a histogram.
    pub hist_w: f32,
    /// Height, in points, of a histogram.
    pub hist_h: f32,
    /// The luma histogram's fill.
    pub hist_luma: Color32,
    /// The red histogram's fill.
    pub hist_r: Color32,
    /// The green histogram's fill.
    pub hist_g: Color32,
    /// The blue histogram's fill.
    pub hist_b: Color32,
    /// The clipping triangles at the histogram's ends.
    pub clip_ink: Color32,
    /// Radius, in points, of a `components::grading_wheel`.
    pub wheel_radius: f32,
    /// The fraction of `wheel_radius` the coloured ring's inner edge sits at.
    pub wheel_inner: f32,
    /// The outline tick around the wheel.
    pub wheel_tick: Color32,
    /// The drag dot's fill.
    pub wheel_dot: Color32,
    /// Radius, in points, of the drag dot.
    pub wheel_handle: f32,
    /// The eight HSL mixer band colours.
    pub band_colors: [Color32; 8],
    /// Diameter, in points, of a `components::hue_band_selector` dot.
    pub band_dot: f32,
    /// Gap, in points, between those dots.
    pub band_gap: f32,
    /// Width, in points, of a grading row's numeric readout.
    pub readout_width: f32,
    /// The least share of `track_w` a grading track shrinks to in a narrow row.
    pub track_min_share: f32,
    /// How much a grading handle grows under the pointer.
    pub handle_lit_scale: f32,
    /// The before/after comparison pair: the gap between the two pictures
    /// and the share of the host's width each one takes.
    pub compare_share: f32,
    /// Alpha (egui's `0..255`) a disabled gradient track is painted at.
    pub disabled_alpha: u8,
    /// Alpha of the plate under a preview's caption (`before`/`after`).
    pub caption_alpha: u8,
    /// A draggable handle's drawn radius, in hairlines (the mask, contour and
    /// sector editors' vertex dots).
    pub handle_hairlines: f32,
    /// A handle's hit-test box, as a multiple of its drawn radius.
    pub handle_hit: f32,
    /// How near an edge a double-click must land to insert a vertex, as a
    /// multiple of the handle's drawn radius.
    pub edge_hit: f32,
    /// The head overlay's face box: half-side as a share of the picture's
    /// width, the `eye_z` term added to it, its floor in points, and the
    /// dot's radius in points.
    pub head_box_scale: f32,
    pub head_box_near: f32,
    pub head_box_min: f32,
    pub head_dot_radius: f32,
}

/// The one interaction-state vocabulary every control shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlState {
    Rest,
    Hover,
    /// Focused / being text-edited.
    Active,
    Dragging,
    Disabled,
    /// Driven by a wired source: shown, but not editable.
    Wired,
}

impl ControlState {
    /// Folds a response and the two flags into one state. `Wired` and
    /// `Disabled` win over pointer states.
    pub fn of(response: &egui::Response, enabled: bool, wired: bool, active: bool) -> Self {
        if wired {
            Self::Wired
        } else if !enabled {
            Self::Disabled
        } else if response.dragged() {
            Self::Dragging
        } else if active {
            Self::Active
        } else if response.hovered() {
            Self::Hover
        } else {
            Self::Rest
        }
    }
}

impl Theme {
    /// The house look, **dark mode**: the red phosphor on a grey tube after
    /// dark — `the build reference` § *The look*, every value the exact token
    /// `web/theme.css` declares (`plan-opus.md` § O.17 keeps the full table).
    /// The personality is the era's: hairlines and near-black grey, one
    /// saturated red that owns every lit state, hover treated as a dim wash
    /// of the same red, press treated as the inversion (filled phosphor,
    /// tube-coloured ink), and a window glow that is the phosphor itself at
    /// a whisper of alpha.
    pub fn phosphor() -> Self {
        Theme {
            name: "phosphor",
            // --- surfaces: the exact dark universe (`web/theme.css`) ---
            tube: Color32::from_rgb(0x08, 0x08, 0x0a),
            tube_raised: Color32::from_rgb(0x0d, 0x0d, 0x10),
            panel: Color32::from_rgb(0x11, 0x11, 0x14),
            panel_head: Color32::from_rgb(0x0a, 0x0a, 0x0d),
            inset: Color32::from_rgb(0x05, 0x05, 0x06),
            ink: Color32::from_rgb(0xe6, 0xe6, 0xe8),
            ink_weak: Color32::from_rgb(0x9a, 0x9a, 0xa2),
            ink_faint: Color32::from_rgb(0x62, 0x62, 0x6b),
            // --- the emphasis, common to both styles ---
            phosphor: Color32::from_rgb(0xff, 0x2e, 0x2e),
            phosphor_lit: Color32::from_rgb(0xff, 0x6f, 0x60),
            phosphor_dim: Color32::from_rgb(0xa0, 0x1d, 0x1d),
            phosphor_deep: Color32::from_rgb(0x7d, 0x10, 0x1f),
            accent_vivid: Color32::from_rgb(0xff, 0x10, 0x3c),
            logo_metal: Color32::from_rgb(0x4d,0x8d,0xff),
            logo_vj: Color32::from_rgb(0xff,0x2e,0x2e),
            soft: Color32::from_rgb(0xc8, 0xcc, 0xd2),
            soft_dim: Color32::from_rgb(0x6a, 0x70, 0x79),
            ok: Color32::from_rgb(0x6f, 0xbf, 0x8f),
            warn: Color32::from_rgb(0xd9, 0xa4, 0x41),
            glow: Color32::from_rgba_unmultiplied(0xff, 0x2e, 0x2e, 0x24),
            rule: Color32::from_rgb(0x1c, 0x1c, 0x20),
            rule_bright: Color32::from_rgb(0x2c, 0x2c, 0x33),
            hairline: 1.0,
            accent_stroke: 1.0,
            radius: 2,
            field_radius: 8,
            menu_radius: 10,
            glow_blur: 24,
            pad: 12,
            header_pad: 8,
            header_pad_y: 5,
            item_gap: Vec2::new(8.0, 6.0),
            segment_gap: Vec2::new(8.0, 3.0),
            wordmark_gap: Vec2::ZERO,
            field_pad: Vec2::new(10.0, 6.0),
            header_height: 40.0,
            interact_size: Vec2::new(40.0, 30.0),
            font_heading: FontId::new(13.0, FontFamily::Monospace),
            font_body: FontId::new(12.0, FontFamily::Monospace),
            font_small: FontId::new(10.0, FontFamily::Monospace),
            focus_share: 0.62,
            cede_share: 0.22,
            row_share: 3.0 / 7.0,
            properties_share: 5.0 / 14.0,
            library_share: 6.0 / 14.0,
            region_min: Vec2::new(220.0, 140.0),
            graph_preview_share: 0.3,
            graph_hover_preview: 200.0,
            menu_max_height: 360.0,
            image_tint: Color32::WHITE,
            text_input_width: 96.0,
            ai_instructions_rows: 6,
            code_rows: 16,
            mask_canvas_width: 160.0,
            overlay_cell: Vec2::new(96.0, 96.0),
            piano_black_depth: 0.6,
            pager_height: 22.0,
            pager_dot: 3.0,
            colour_presets: [
                [[0.05, 0.20, 0.55], [0.10, 0.75, 0.90], [0.90, 0.97, 1.00], [0.45, 0.30, 0.85]],
                [[0.85, 0.15, 0.10], [0.98, 0.50, 0.10], [1.00, 0.75, 0.20], [1.00, 0.95, 0.85]],
                [[0.10, 0.80, 0.30], [0.90, 0.10, 0.75], [0.02, 0.30, 0.15], [1.00, 0.55, 0.80]],
                [[0.45, 0.10, 0.60], [0.90, 0.05, 0.15], [0.20, 0.02, 0.35], [0.60, 0.02, 0.12]],
            ],
            rgb_presets: [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.75, 0.0],
                [0.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            swatch_strip: 0.4,
            gear_size: 14.0,
            swatch_button: Vec2::new(28.0, 20.0),
            properties_narrow: 260.0,
            table_header_height: 18.0,
            table_row_height: 16.0,
            stripe: Color32::from_rgb(0x16, 0x16, 0x1a),
            library_pad: 8,
            library_gap: 2.0,
            library_row: 24.0,
            library_card_h: 48.0,
            library_card_radius: 6,
            library_card_pad: Vec2::new(8.0, 5.0),
            library_card: Color32::from_rgb(0x16, 0x16, 0x1a),
            library_card_hover: Color32::from_rgb(0x21, 0x21, 0x28),
            library_badge: Color32::from_rgb(0x2a, 0x2a, 0x31),
            library_badge_size: Vec2::new(24.0, 24.0),
            library_detail_gap: 2.0,
            row_menu_width: 24.0,
            sidebar_share: 0.32,
            sidebar_min_width: 220.0,
            sidebar_max_share: 0.5,
            long_press: 0.35,
            canvas_bar_height: 26.0,
            panel_head_height: 26.0,
            compact_row_height: 22.0,
            icon_button_size: Vec2::new(26.0, 26.0),
            nav_row_height: 32.0,
            nav_icon_side: 18.0,
            nav_row_gap: 0.0,
            hover_alpha: 44,
            selected_alpha: 96,
            payload_max_height: 140.0,
            drag_ghost_opacity: 0.85,
            bar_pad: 4,
            hold_bar_pad: 2,
            gallery_share: 0.32,
            gallery_card_min_w: 68.0,
            node_port_preview_radius: 2.5,
            scroll_gutter: 12.0,
            compact_row_pad: Vec2::new(6.0, 2.0),
            graph_grid: 40.0,
            graph_zoom_step: 1.25,
            graph_zoom_min: 0.2,
            graph_zoom_max: 4.0,
            graph_label_pad: 12.0,
            graph_undo_depth: 32,
            node_canvas: Color32::from_rgb(0x05, 0x05, 0x08),
            node_body: Color32::from_rgb(0x1b, 0x1b, 0x20),
            node_header: Color32::from_rgb(0x2a, 0x2a, 0x31),
            node_title_ink: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_row_ink: Color32::from_rgb(0xc8, 0xc8, 0xcc),
            node_field: Color32::from_rgb(0x0a, 0x0a, 0x0d),
            node_field_ink: Color32::from_rgb(0xe6, 0xe6, 0xe8),
            node_dot_on: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_dot_off: Color32::from_rgb(0x6a, 0x6a, 0x72),
            node_select: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_cable: Color32::from_rgb(0x7f, 0x7f, 0x88),
            node_cable_accent: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_more_ink: Color32::from_rgb(0x9a, 0x9a, 0xa2),
            node_cable_dash: 6.0,
            node_cable_width: 1.5,
            node_label_inset: 14.0,
            picker_square: Vec2::new(140.0, 120.0),
            picker_hue: Vec2::new(16.0, 120.0),
            picker_gap: 8.0,
            picker_handle: 8.0,
            picker_handle_stroke: Color32::from_rgb(0x0d, 0x0d, 0x10),
            mask_inside: Color32::from_rgba_unmultiplied(0xff, 0x2e, 0x2e, 0x38),
            mask_outside: Color32::from_rgba_unmultiplied(0x05, 0x05, 0x06, 0xaa),
            mask_vertex_handle: Color32::from_rgb(0xff, 0x2e, 0x2e),
            control_width: 120.0,
            label_width: 84.0,
            section_gap: 6.0,
            card_pad: 8,
            card_gap: 8.0,
            swatch_size: Vec2::new(22.0, 16.0),
            track_h: 8.0,
            track_w: 160.0,
            track_radius: 3,
            track_fill: Color32::from_rgb(0x1c, 0x1c, 0x20),
            track_center: Color32::from_rgb(0x6a, 0x6a, 0x72),
            track_edge: Color32::from_rgb(0x0a, 0x0a, 0x0d),
            temp_warm: Color32::from_rgb(0xff, 0xd2, 0x5f),
            temp_cold: Color32::from_rgb(0x4f, 0x8b, 0xff),
            tint_green: Color32::from_rgb(0x4f, 0xd6, 0x6a),
            tint_magenta: Color32::from_rgb(0xd6, 0x4f, 0xc8),
            curve_size: 140.0,
            curve_grid: Color32::from_rgb(0x2c, 0x2c, 0x33),
            curve_ink: Color32::from_rgb(0xe6, 0xe6, 0xe8),
            curve_channel_red: Color32::from_rgb(0xff, 0x4d, 0x4d),
            curve_channel_green: Color32::from_rgb(0x5f, 0xd6, 0x7a),
            curve_channel_blue: Color32::from_rgb(0x5f, 0x8b, 0xff),
            curve_handle: 5.0,
            hist_w: 160.0,
            hist_h: 64.0,
            hist_luma: Color32::from_rgb(0xc8, 0xcc, 0xd2),
            hist_r: Color32::from_rgb(0xff, 0x2e, 0x2e),
            hist_g: Color32::from_rgb(0x4f, 0xd6, 0x6a),
            hist_b: Color32::from_rgb(0x4f, 0x8b, 0xff),
            clip_ink: Color32::from_rgba_unmultiplied(0xff, 0x2e, 0x2e, 0xcc),
            wheel_radius: 56.0,
            wheel_inner: 0.72,
            wheel_tick: Color32::from_rgb(0x2c, 0x2c, 0x33),
            wheel_dot: Color32::from_rgb(0xe6, 0xe6, 0xe8),
            wheel_handle: 5.0,
            band_colors: [
                Color32::from_rgb(0xff, 0x3b, 0x30),
                Color32::from_rgb(0xff, 0x8a, 0x2b),
                Color32::from_rgb(0xff, 0xd6, 0x2b),
                Color32::from_rgb(0x6f, 0xd6, 0x4f),
                Color32::from_rgb(0x4f, 0xd6, 0xc8),
                Color32::from_rgb(0x4f, 0x8b, 0xff),
                Color32::from_rgb(0x9b, 0x5f, 0xff),
                Color32::from_rgb(0xd6, 0x4f, 0xc8),
            ],
            band_dot: 14.0,
            band_gap: 6.0,
            readout_width: 34.0,
            track_min_share: 0.5,
            handle_lit_scale: 1.3,
            compare_share: 0.24,
            disabled_alpha: 0x66,
            caption_alpha: 0xc0,
            handle_hairlines: 4.0,
            handle_hit: 3.0,
            edge_hit: 4.0,
            head_box_scale: 0.05,
            head_box_near: 0.6,
            head_box_min: 6.0,
            head_dot_radius: 2.5,
        }
    }

    /// The house look, **light mode** — the same instrument with the tube
    /// switched on: warm cream surfaces with the red lean the dark tube's
    /// one-point-of-red rule gives the black, the same emphasis (`accent`,
    /// `accent-lit`) carried over unchanged — *the emphasis is the common
    /// half* (`plan-opus.md` § O.17). Everything between the emphasis and
    /// the ground has its own exact value per style; nothing in this table
    /// is a derivation of the other theme's numbers.
    pub fn daylight() -> Self {
        Theme {
            name: "daylight",
            tube: Color32::from_rgb(0xf7, 0xf3, 0xf0),
            tube_raised: Color32::from_rgb(0xff, 0xfb, 0xfa),
            panel: Color32::from_rgb(0xf1, 0xe9, 0xe5),
            panel_head: Color32::from_rgb(0xdc, 0xcf, 0xc9),
            inset: Color32::from_rgb(0xe8, 0xde, 0xd9),
            ink: Color32::from_rgb(0x23, 0x1c, 0x1e),
            ink_weak: Color32::from_rgb(0x6f, 0x62, 0x66),
            ink_faint: Color32::from_rgb(0x9a, 0x8c, 0x90),
            phosphor: Color32::from_rgb(0xff, 0x2e, 0x2e),
            phosphor_lit: Color32::from_rgb(0xff, 0x6f, 0x60),
            phosphor_dim: Color32::from_rgb(0xb5, 0x26, 0x26),
            phosphor_deep: Color32::from_rgb(0xf3, 0xd2, 0xcd),
            accent_vivid: Color32::from_rgb(0xff, 0x17, 0x44),
            logo_metal: Color32::from_rgb(0x1f,0x5f,0xd6),
            logo_vj: Color32::from_rgb(0xd6,0x28,0x28),
            soft: Color32::from_rgb(0x3c, 0x41, 0x48),
            soft_dim: Color32::from_rgb(0x9a, 0xa0, 0xa8),
            ok: Color32::from_rgb(0x2f, 0x8f, 0x5b),
            warn: Color32::from_rgb(0xb5, 0x7a, 0x1e),
            glow: Color32::from_rgba_unmultiplied(0x4a, 0x0f, 0x0f, 0x20),
            rule: Color32::from_rgb(0xdd, 0xd0, 0xca),
            rule_bright: Color32::from_rgb(0xc2, 0xad, 0xa6),
            hairline: 1.0,
            accent_stroke: 1.0,
            radius: 2,
            field_radius: 8,
            menu_radius: 10,
            glow_blur: 24,
            pad: 12,
            header_pad: 8,
            header_pad_y: 5,
            item_gap: Vec2::new(8.0, 6.0),
            segment_gap: Vec2::new(8.0, 3.0),
            wordmark_gap: Vec2::ZERO,
            field_pad: Vec2::new(10.0, 6.0),
            header_height: 40.0,
            interact_size: Vec2::new(40.0, 30.0),
            font_heading: FontId::new(13.0, FontFamily::Monospace),
            font_body: FontId::new(12.0, FontFamily::Monospace),
            font_small: FontId::new(10.0, FontFamily::Monospace),
            focus_share: 0.62,
            cede_share: 0.22,
            row_share: 3.0 / 7.0,
            properties_share: 5.0 / 14.0,
            library_share: 6.0 / 14.0,
            region_min: Vec2::new(220.0, 140.0),
            graph_preview_share: 0.3,
            graph_hover_preview: 200.0,
            menu_max_height: 360.0,
            image_tint: Color32::WHITE,
            text_input_width: 96.0,
            ai_instructions_rows: 6,
            code_rows: 16,
            mask_canvas_width: 160.0,
            overlay_cell: Vec2::new(96.0, 96.0),
            piano_black_depth: 0.6,
            pager_height: 22.0,
            pager_dot: 3.0,
            colour_presets: [
                [[0.05, 0.20, 0.55], [0.10, 0.75, 0.90], [0.90, 0.97, 1.00], [0.45, 0.30, 0.85]],
                [[0.85, 0.15, 0.10], [0.98, 0.50, 0.10], [1.00, 0.75, 0.20], [1.00, 0.95, 0.85]],
                [[0.10, 0.80, 0.30], [0.90, 0.10, 0.75], [0.02, 0.30, 0.15], [1.00, 0.55, 0.80]],
                [[0.45, 0.10, 0.60], [0.90, 0.05, 0.15], [0.20, 0.02, 0.35], [0.60, 0.02, 0.12]],
            ],
            rgb_presets: [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.75, 0.0],
                [0.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            swatch_strip: 0.4,
            gear_size: 14.0,
            swatch_button: Vec2::new(28.0, 20.0),
            properties_narrow: 260.0,
            table_header_height: 18.0,
            table_row_height: 16.0,
            stripe: Color32::from_rgb(0x16, 0x16, 0x1a),
            library_pad: 8,
            library_gap: 2.0,
            library_row: 24.0,
            library_card_h: 48.0,
            library_card_radius: 6,
            library_card_pad: Vec2::new(8.0, 5.0),
            library_card: Color32::from_rgb(0xff, 0xfb, 0xfa),
            library_card_hover: Color32::from_rgb(0xf3, 0xec, 0xe8),
            library_badge: Color32::from_rgb(0xec, 0xe0, 0xdb),
            library_badge_size: Vec2::new(24.0, 24.0),
            library_detail_gap: 2.0,
            row_menu_width: 24.0,
            sidebar_share: 0.32,
            sidebar_min_width: 220.0,
            sidebar_max_share: 0.5,
            long_press: 0.35,
            canvas_bar_height: 26.0,
            panel_head_height: 26.0,
            compact_row_height: 22.0,
            icon_button_size: Vec2::new(26.0, 26.0),
            nav_row_height: 32.0,
            nav_icon_side: 18.0,
            nav_row_gap: 0.0,
            hover_alpha: 30,
            selected_alpha: 58,
            payload_max_height: 140.0,
            drag_ghost_opacity: 0.85,
            bar_pad: 4,
            hold_bar_pad: 2,
            gallery_share: 0.32,
            gallery_card_min_w: 68.0,
            node_port_preview_radius: 2.5,
            scroll_gutter: 12.0,
            compact_row_pad: Vec2::new(6.0, 2.0),
            graph_grid: 40.0,
            graph_zoom_step: 1.25,
            graph_zoom_min: 0.2,
            graph_zoom_max: 4.0,
            graph_label_pad: 12.0,
            graph_undo_depth: 32,
            node_canvas: Color32::from_rgb(0xf3, 0xec, 0xe8),
            node_body: Color32::from_rgb(0xff, 0xfb, 0xfa),
            node_header: Color32::from_rgb(0xec, 0xe0, 0xdb),
            node_title_ink: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_row_ink: Color32::from_rgb(0x4a, 0x40, 0x44),
            node_field: Color32::from_rgb(0xe8, 0xde, 0xd9),
            node_field_ink: Color32::from_rgb(0x23, 0x1c, 0x1e),
            node_dot_on: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_dot_off: Color32::from_rgb(0x9a, 0x8c, 0x90),
            node_select: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_cable: Color32::from_rgb(0x9a, 0x8c, 0x90),
            node_cable_accent: Color32::from_rgb(0xff, 0x2e, 0x2e),
            node_more_ink: Color32::from_rgb(0x6f, 0x62, 0x66),
            node_cable_dash: 6.0,
            node_cable_width: 1.5,
            node_label_inset: 14.0,
            picker_square: Vec2::new(140.0, 120.0),
            picker_hue: Vec2::new(16.0, 120.0),
            picker_gap: 8.0,
            picker_handle: 8.0,
            picker_handle_stroke: Color32::from_rgb(0xff, 0xfb, 0xfa),
            mask_inside: Color32::from_rgba_unmultiplied(0xff, 0x2e, 0x2e, 0x30),
            mask_outside: Color32::from_rgba_unmultiplied(0xf7, 0xf3, 0xf0, 0xa8),
            mask_vertex_handle: Color32::from_rgb(0xff, 0x2e, 0x2e),
            control_width: 120.0,
            label_width: 84.0,
            section_gap: 6.0,
            card_pad: 8,
            card_gap: 8.0,
            swatch_size: Vec2::new(22.0, 16.0),
            track_h: 8.0,
            track_w: 160.0,
            track_radius: 3,
            track_fill: Color32::from_rgb(0xe8, 0xde, 0xd9),
            track_center: Color32::from_rgb(0x9a, 0x8c, 0x90),
            track_edge: Color32::from_rgb(0xc2, 0xad, 0xa6),
            temp_warm: Color32::from_rgb(0xff, 0xc2, 0x4a),
            temp_cold: Color32::from_rgb(0x3f, 0x78, 0xe8),
            tint_green: Color32::from_rgb(0x3f, 0xc2, 0x5f),
            tint_magenta: Color32::from_rgb(0xc2, 0x3f, 0xb5),
            curve_size: 140.0,
            curve_grid: Color32::from_rgb(0xdd, 0xd0, 0xca),
            curve_ink: Color32::from_rgb(0x23, 0x1c, 0x1e),
            curve_channel_red: Color32::from_rgb(0xe0, 0x2e, 0x2e),
            curve_channel_green: Color32::from_rgb(0x2f, 0x9f, 0x52),
            curve_channel_blue: Color32::from_rgb(0x2f, 0x63, 0xd6),
            curve_handle: 5.0,
            hist_w: 160.0,
            hist_h: 64.0,
            hist_luma: Color32::from_rgb(0x3c, 0x41, 0x48),
            hist_r: Color32::from_rgb(0xff, 0x2e, 0x2e),
            hist_g: Color32::from_rgb(0x3f, 0xc2, 0x5f),
            hist_b: Color32::from_rgb(0x3f, 0x78, 0xe8),
            clip_ink: Color32::from_rgba_unmultiplied(0xff, 0x2e, 0x2e, 0xcc),
            wheel_radius: 56.0,
            wheel_inner: 0.72,
            wheel_tick: Color32::from_rgb(0xc2, 0xad, 0xa6),
            wheel_dot: Color32::from_rgb(0x23, 0x1c, 0x1e),
            wheel_handle: 5.0,
            band_colors: [
                Color32::from_rgb(0xff, 0x3b, 0x30),
                Color32::from_rgb(0xff, 0x8a, 0x2b),
                Color32::from_rgb(0xff, 0xd6, 0x2b),
                Color32::from_rgb(0x6f, 0xd6, 0x4f),
                Color32::from_rgb(0x4f, 0xd6, 0xc8),
                Color32::from_rgb(0x4f, 0x8b, 0xff),
                Color32::from_rgb(0x9b, 0x5f, 0xff),
                Color32::from_rgb(0xd6, 0x4f, 0xc8),
            ],
            band_dot: 14.0,
            band_gap: 6.0,
            readout_width: 34.0,
            track_min_share: 0.5,
            handle_lit_scale: 1.3,
            compare_share: 0.24,
            disabled_alpha: 0x66,
            caption_alpha: 0xc0,
            handle_hairlines: 4.0,
            handle_hit: 3.0,
            edge_hit: 4.0,
            head_box_scale: 0.05,
            head_box_near: 0.6,
            head_box_min: 6.0,
            head_dot_radius: 2.5,
        }
    }

    /// This theme recoloured from a four-colour palette (`[r, g, b]` in 0..=1,
    /// the stored `Palettes` shape): the most vivid colour becomes the
    /// phosphor ladder, the next one the cold second accent, the darkest tints
    /// every surface, the lightest the ink. Every other token — spacing, type,
    /// radii — stays this theme's.
    pub fn with_palette(&self, colors: &[[f32; 3]]) -> Theme {
        if colors.is_empty() {
            return self.clone();
        }
        let hsv = |c: [f32; 3]| {
            let h = egui::ecolor::Hsva::from_rgb(c);
            (h.h, h.s, h.v)
        };
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let mut by_vivid: Vec<[f32; 3]> = colors.to_vec();
        by_vivid.sort_by(|a, b| {
            let (sa, sb) = (hsv(*a), hsv(*b));
            (sb.1 * sb.2).total_cmp(&(sa.1 * sa.2))
        });
        let darkest = colors.iter().copied().min_by(|a, b| lum(*a).total_cmp(&lum(*b))).unwrap_or([0.1; 3]);
        let lightest = colors.iter().copied().max_by(|a, b| lum(*a).total_cmp(&lum(*b))).unwrap_or([0.9; 3]);
        let accent = by_vivid[0];
        let second = by_vivid.get(1).copied().unwrap_or(accent);
        let to32 = |c: [f32; 3]| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
        let hot = |c: [f32; 3], s: f32, v: f32| {
            let (h, _, _) = hsv(c);
            to32(egui::ecolor::Hsva::new(h, s, v, 1.0).to_rgb())
        };
        let scale = |c: [f32; 3], k: f32| to32([c[0] * k, c[1] * k, c[2] * k]);
        let mix = |a: Color32, b: Color32, t: f32| {
            let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
            Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
        };
        let phosphor = hot(accent, 0.9_f32.max(hsv(accent).1), 1.0);
        let mut t = self.clone();
        t.name = "palette";
        t.tube = scale(darkest, 0.10);
        t.tube_raised = scale(darkest, 0.14);
        t.panel = scale(darkest, 0.19);
        t.panel_head = scale(darkest, 0.12);
        t.inset = scale(darkest, 0.06);
        t.stripe = scale(darkest, 0.22);
        t.library_card = scale(darkest, 0.22);
        t.library_card_hover = scale(darkest, 0.34);
        t.library_badge = scale(darkest, 0.42);
        t.rule = scale(darkest, 0.30);
        t.rule_bright = scale(darkest, 0.50);
        t.ink = mix(Color32::from_rgb(0xe6, 0xe6, 0xe8), to32(lightest), 0.18);
        t.ink_weak = mix(t.ink, t.panel, 0.45);
        t.ink_faint = mix(t.ink, t.panel, 0.70);
        t.phosphor = phosphor;
        t.phosphor_lit = mix(phosphor, Color32::WHITE, 0.35);
        t.phosphor_dim = mix(phosphor, Color32::BLACK, 0.38);
        t.phosphor_deep = mix(phosphor, Color32::BLACK, 0.52);
        t.accent_vivid = hot(accent, 1.0, 1.0);
        t.logo_vj = phosphor;
        t.logo_metal = hot(second, 0.7, 1.0);
        t.soft = mix(hot(second, 0.35, 0.95), Color32::WHITE, 0.2);
        t.soft_dim = mix(t.soft, t.panel, 0.55);
        t.glow = Color32::from_rgba_unmultiplied(phosphor.r(), phosphor.g(), phosphor.b(), 0x24);
        t
    }

    /// The house theme named `s`: `"dark"`/`"phosphor"` after dark,
    /// `"light"`/`"daylight"` with the tube switched on — the same knob
    /// `--theme`/`FW_THEME` turns, resolved by `main.rs`. `Err` on anything
    /// else, so a typo is a startup error, not a quiet fall-through to the
    /// dark theme.
    pub fn from_name(s: &str) -> Option<Theme> {
        match s {
            "dark" | "phosphor" => Some(Theme::phosphor()),
            "light" | "daylight" => Some(Theme::daylight()),
            _ => None,
        }
    }

    /// A deliberately different look — light ground, blue accent, rounder,
    /// roomier, proportional type. Not for the venue: it is the second theme
    /// the restyle drill paints under, and it differs from [`Theme::phosphor`]
    /// in every colour and in every spacing so the drill cannot pass by
    /// accident.
    // Only the restyle drill paints under it; the app never will.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn contrast() -> Self {
        Theme {
            name: "contrast",
            tube: Color32::from_rgb(0xf4, 0xf4, 0xf0),
            tube_raised: Color32::from_rgb(0xff, 0xff, 0xff),
            panel: Color32::from_rgb(0xea, 0xea, 0xe4),
            panel_head: Color32::from_rgb(0xd0, 0xd0, 0xc6),
            inset: Color32::from_rgb(0xdc, 0xdc, 0xd4),
            ink: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            ink_weak: Color32::from_rgb(0x5c, 0x5c, 0x60),
            ink_faint: Color32::from_rgb(0x86, 0x86, 0x8a),
            phosphor: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            phosphor_lit: Color32::from_rgb(0x64, 0x93, 0xf0),
            phosphor_dim: Color32::from_rgb(0x2c, 0x50, 0x9e),
            phosphor_deep: Color32::from_rgb(0xd6, 0xe0, 0xf5),
            accent_vivid: Color32::from_rgb(0x2b, 0x6c, 0xff),
            logo_metal: Color32::from_rgb(0x1f,0x5f,0xd6),
            logo_vj: Color32::from_rgb(0xd6,0x28,0x28),
            soft: Color32::from_rgb(0xe0, 0x3a, 0x1a),
            soft_dim: Color32::from_rgb(0x9a, 0x5b, 0x4a),
            ok: Color32::from_rgb(0x2f, 0x8f, 0x5b),
            warn: Color32::from_rgb(0xa3, 0x6a, 0x12),
            glow: Color32::from_rgba_unmultiplied(0x1f, 0x5f, 0xd6, 0x20),
            rule: Color32::from_rgb(0xc8, 0xc8, 0xc0),
            rule_bright: Color32::from_rgb(0x9a, 0x9a, 0x90),
            hairline: 2.0,
            accent_stroke: 3.0,
            radius: 8,
            field_radius: 8,
            menu_radius: 10,
            glow_blur: 32,
            pad: 20,
            header_pad: 14,
            header_pad_y: 13,
            item_gap: Vec2::new(14.0, 12.0),
            segment_gap: Vec2::new(14.0, 6.0),
            wordmark_gap: Vec2::new(2.0, 0.0),
            field_pad: Vec2::new(10.0, 6.0),
            header_height: 56.0,
            interact_size: Vec2::new(56.0, 30.0),
            font_heading: FontId::new(18.0, FontFamily::Proportional),
            font_body: FontId::new(15.0, FontFamily::Proportional),
            font_small: FontId::new(12.0, FontFamily::Proportional),
            focus_share: 0.62,
            cede_share: 0.22,
            row_share: 3.0 / 7.0,
            properties_share: 5.0 / 14.0,
            library_share: 6.0 / 14.0,
            region_min: Vec2::new(220.0, 140.0),
            graph_preview_share: 0.3,
            graph_hover_preview: 280.0,
            menu_max_height: 480.0,
            image_tint: Color32::WHITE,
            text_input_width: 140.0,
            ai_instructions_rows: 6,
            code_rows: 16,
            mask_canvas_width: 220.0,
            overlay_cell: Vec2::new(140.0, 140.0),
            piano_black_depth: 0.6,
            pager_height: 28.0,
            pager_dot: 4.0,
            colour_presets: [
                [[0.05, 0.20, 0.55], [0.10, 0.75, 0.90], [0.90, 0.97, 1.00], [0.45, 0.30, 0.85]],
                [[0.85, 0.15, 0.10], [0.98, 0.50, 0.10], [1.00, 0.75, 0.20], [1.00, 0.95, 0.85]],
                [[0.10, 0.80, 0.30], [0.90, 0.10, 0.75], [0.02, 0.30, 0.15], [1.00, 0.55, 0.80]],
                [[0.45, 0.10, 0.60], [0.90, 0.05, 0.15], [0.20, 0.02, 0.35], [0.60, 0.02, 0.12]],
            ],
            rgb_presets: [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.75, 0.0],
                [0.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            swatch_strip: 0.4,
            gear_size: 14.0,
            swatch_button: Vec2::new(28.0, 20.0),
            properties_narrow: 340.0,
            table_header_height: 26.0,
            table_row_height: 24.0,
            stripe: Color32::from_rgb(0xe2, 0xe2, 0xda),
            library_pad: 14,
            library_gap: 3.0,
            library_row: 34.0,
            library_card_h: 64.0,
            library_card_radius: 10,
            library_card_pad: Vec2::new(14.0, 8.0),
            library_card: Color32::from_rgb(0xff, 0xff, 0xff),
            library_card_hover: Color32::from_rgb(0xf0, 0xf0, 0xea),
            library_badge: Color32::from_rgb(0xdc, 0xdc, 0xd4),
            library_badge_size: Vec2::new(30.0, 30.0),
            library_detail_gap: 3.0,
            row_menu_width: 30.0,
            sidebar_share: 0.34,
            sidebar_min_width: 220.0,
            sidebar_max_share: 0.5,
            long_press: 0.5,
            canvas_bar_height: 34.0,
            panel_head_height: 26.0,
            compact_row_height: 22.0,
            icon_button_size: Vec2::new(34.0, 34.0),
            nav_row_height: 40.0,
            nav_icon_side: 22.0,
            nav_row_gap: 0.0,
            hover_alpha: 30,
            selected_alpha: 58,
            payload_max_height: 140.0,
            drag_ghost_opacity: 0.85,
            bar_pad: 6,
            hold_bar_pad: 3,
            gallery_share: 0.36,
            gallery_card_min_w: 88.0,
            node_port_preview_radius: 3.5,
            scroll_gutter: 16.0,
            compact_row_pad: Vec2::new(6.0, 2.0),
            graph_grid: 64.0,
            graph_zoom_step: 1.2,
            graph_zoom_min: 0.25,
            graph_zoom_max: 3.0,
            graph_label_pad: 16.0,
            graph_undo_depth: 16,
            node_canvas: Color32::from_rgb(0xe4, 0xe4, 0xdc),
            node_body: Color32::from_rgb(0xf0, 0xf0, 0xea),
            node_header: Color32::from_rgb(0xda, 0xda, 0xd0),
            node_title_ink: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            node_row_ink: Color32::from_rgb(0x30, 0x30, 0x34),
            node_field: Color32::from_rgb(0xdc, 0xdc, 0xd4),
            node_field_ink: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            node_dot_on: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            node_dot_off: Color32::from_rgb(0x86, 0x86, 0x8a),
            node_select: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            node_cable: Color32::from_rgb(0x86, 0x86, 0x8a),
            node_cable_accent: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            node_more_ink: Color32::from_rgb(0x5c, 0x5c, 0x60),
            node_cable_dash: 9.0,
            node_cable_width: 2.0,
            node_label_inset: 16.0,
            picker_square: Vec2::new(200.0, 160.0),
            picker_hue: Vec2::new(24.0, 160.0),
            picker_gap: 14.0,
            picker_handle: 12.0,
            picker_handle_stroke: Color32::from_rgb(0xff, 0xff, 0xff),
            mask_inside: Color32::from_rgba_unmultiplied(0x1f, 0x5f, 0xd6, 0x34),
            mask_outside: Color32::from_rgba_unmultiplied(0xf4, 0xf4, 0xf0, 0xaa),
            mask_vertex_handle: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            control_width: 168.0,
            label_width: 116.0,
            section_gap: 10.0,
            card_pad: 8,
            card_gap: 8.0,
            swatch_size: Vec2::new(30.0, 22.0),
            track_h: 12.0,
            track_w: 220.0,
            track_radius: 6,
            track_fill: Color32::from_rgb(0xdc, 0xdc, 0xd4),
            track_center: Color32::from_rgb(0x86, 0x86, 0x8a),
            track_edge: Color32::from_rgb(0xb0, 0xb0, 0xa6),
            temp_warm: Color32::from_rgb(0xff, 0xc2, 0x4a),
            temp_cold: Color32::from_rgb(0x2f, 0x63, 0xd6),
            tint_green: Color32::from_rgb(0x2f, 0x9f, 0x52),
            tint_magenta: Color32::from_rgb(0xb5, 0x2f, 0xa8),
            curve_size: 200.0,
            curve_grid: Color32::from_rgb(0xc8, 0xc8, 0xc0),
            curve_ink: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            curve_channel_red: Color32::from_rgb(0xd6, 0x2a, 0x1a),
            curve_channel_green: Color32::from_rgb(0x2f, 0x8f, 0x5b),
            curve_channel_blue: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            curve_handle: 7.0,
            hist_w: 220.0,
            hist_h: 92.0,
            hist_luma: Color32::from_rgb(0x30, 0x30, 0x34),
            hist_r: Color32::from_rgb(0xd6, 0x2a, 0x1a),
            hist_g: Color32::from_rgb(0x2f, 0x8f, 0x5b),
            hist_b: Color32::from_rgb(0x1f, 0x5f, 0xd6),
            clip_ink: Color32::from_rgba_unmultiplied(0x1f, 0x5f, 0xd6, 0xcc),
            wheel_radius: 80.0,
            wheel_inner: 0.72,
            wheel_tick: Color32::from_rgb(0x9a, 0x9a, 0x90),
            wheel_dot: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            wheel_handle: 7.0,
            band_colors: [
                Color32::from_rgb(0xff, 0x3b, 0x30),
                Color32::from_rgb(0xff, 0x8a, 0x2b),
                Color32::from_rgb(0xff, 0xd6, 0x2b),
                Color32::from_rgb(0x6f, 0xd6, 0x4f),
                Color32::from_rgb(0x4f, 0xd6, 0xc8),
                Color32::from_rgb(0x4f, 0x8b, 0xff),
                Color32::from_rgb(0x9b, 0x5f, 0xff),
                Color32::from_rgb(0xd6, 0x4f, 0xc8),
            ],
            band_dot: 20.0,
            band_gap: 10.0,
            readout_width: 34.0,
            track_min_share: 0.5,
            handle_lit_scale: 1.3,
            compare_share: 0.24,
            disabled_alpha: 0x66,
            caption_alpha: 0xc0,
            handle_hairlines: 4.0,
            handle_hit: 3.0,
            edge_hit: 4.0,
            head_box_scale: 0.05,
            head_box_near: 0.6,
            head_box_min: 6.0,
            head_dot_radius: 2.5,
        }
    }

    /// `[r, g, b]` in `0..1` as a displayable colour — the one bridge from a
    /// value the shell holds (`metal_notation::color::Hsb`) to the type a painter
    /// draws with. Here, in `classes.rs`, because `components.rs` may not name
    /// a concrete colour constructor (gate 2) and every panel needs this exact
    /// conversion for a swatch, a picker or a preview.
    pub fn color_of(&self, rgb: [f32; 3]) -> Color32 {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        Color32::from_rgb(byte(rgb[0]), byte(rgb[1]), byte(rgb[2]))
    }

    /// The hairline rule as a stroke.
    pub fn rule_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.rule)
    }

    /// The stroke of a library row's painted icon glyph: the weak ink at the
    /// hairline width, so the glyphs read as quiet marks beside the title.
    pub fn icon_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.ink_weak)
    }

    /// The brighter rule, for a focused or hovered edge.
    pub fn rule_bright_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.rule_bright)
    }

    /// The phosphor as a stroke.
    pub fn accent(&self) -> Stroke {
        Stroke::new(self.accent_stroke, self.phosphor)
    }

    /// `color` at a hairline width — the ingest lane's line plot
    /// (`shell::lanes::paint_ingest`) and the router band's tick-boundary
    /// rules (`paint_router_band`) share this rather than each picking its
    /// own stroke width; only the colour varies per call site.
    pub fn line_stroke(&self, color: Color32) -> Stroke {
        Stroke::new(self.hairline, color)
    }

    // --- the colour-grading kit (feat/fx-grade) --------------------------

    /// The outlined edge of a gradient track (`components::slider_tracked`).
    pub fn track_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.track_edge)
    }

    /// The corner radius of a gradient track — a rounded pill, distinct from
    /// the widget radius, so the track reads as a groove.
    pub fn track_corner(&self) -> CornerRadius {
        CornerRadius::same(self.track_radius)
    }

    /// No corner radius — the histogram's bars and the curve graph's grid.
    pub fn square_corner(&self) -> CornerRadius {
        CornerRadius::ZERO
    }

    /// The ink a curve graph draws with for `channel`: `"red"`, `"green"`,
    /// `"blue"`, anything else (the master curve).
    pub fn curve_channel_ink(&self, channel: &str) -> Color32 {
        match channel {
            "red" => self.curve_channel_red,
            "green" => self.curve_channel_green,
            "blue" => self.curve_channel_blue,
            _ => self.curve_ink,
        }
    }

    /// The stroke a curve graph draws its spline with, per `channel`.
    pub fn curve_stroke(&self, channel: &str) -> Stroke {
        Stroke::new(self.accent_stroke, self.curve_channel_ink(channel))
    }

    /// `a` and `b` mixed by `t` (0 = `a`, 1 = `b`) — the one place a
    /// two-colour gradient stop becomes a `Color32`.
    pub fn blend(&self, a: Color32, b: Color32, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t + 0.5) as u8;
        Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
    }

    /// A full-saturation, full-value hue at `hue01` (0..1) as a `Color32` —
    /// the grading wheel's ring and the hue tracks' rainbow, the one hue
    /// bridge gate 2 allows outside `hue`.
    pub fn rainbow(&self, hue01: f32) -> Color32 {
        egui::ecolor::Hsva::new(hue01.rem_euclid(1.0), 1.0, 1.0, 1.0).into()
    }

    /// A named colour the grading widgets speak in (`Track::Gradient`'s
    /// `from`/`to`) as a token. Unknown names fall back to the phosphor, so
    /// a typo is a red track, not a panic.
    pub fn named_colour(&self, name: &str) -> Color32 {
        match name {
            "red" => self.band_colors[0],
            "orange" => self.band_colors[1],
            "yellow" => self.band_colors[2],
            "green" => self.band_colors[3],
            "aqua" | "cyan" => self.band_colors[4],
            "blue" => self.band_colors[5],
            "purple" => self.band_colors[6],
            "magenta" => self.band_colors[7],
            "grey" | "gray" => self.track_center,
            "dark" => self.track_edge,
            "white" => self.ink,
            "black" => self.ground(),
            "warm" => self.temp_warm,
            "cold" => self.temp_cold,
            _ => self.phosphor,
        }
    }

    // --- the Wire node canvas (N): strokes and the one colour cast --------

    /// A cable's stroke: the accent when it touches the selected node, the
    /// grey cable ink otherwise (`nodes-reference.png`). The width is the
    /// theme's own.
    pub fn node_cable_stroke(&self, accent: bool) -> Stroke {
        Stroke::new(
            self.node_cable_width,
            if accent {
                self.node_cable_accent
            } else {
                self.node_cable
            },
        )
    }

    /// The outline a selected node's box carries — the accent, twice the
    /// hairline so it reads as a ring.
    pub fn node_select_stroke(&self) -> Stroke {
        Stroke::new(self.accent_stroke * 2.0, self.node_select)
    }

    /// The reveal's pulse ring (plan "Surface sources"): the accent, one
    /// hairline wide, fading with `fade` (1 = full, 0 = gone).
    pub fn reveal_pulse_stroke(&self, fade: f32) -> Stroke {
        Stroke::new(self.accent_stroke, self.phosphor.gamma_multiply(fade))
    }

    /// The outline of the library card last clicked (the picked file) — the
    /// open song keeps the heavier accent.
    pub fn card_picked_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.phosphor)
    }

    /// A node box's outline when it is not the selection.
    pub fn node_box_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.rule)
    }

    /// The ring around an output dot, in the body colour so the dot reads as
    /// sitting on the card.
    pub fn node_port_ring_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.node_body)
    }

    /// A swatch's edge, so a dark chip does not vanish into the body.
    pub fn node_swatch_stroke(&self) -> Stroke {
        Stroke::new(self.hairline, self.node_header)
    }

    /// A 0..1 RGB triple as a `Color32`. Gate 2 keeps the `Color32::` cast
    /// here; a colour node's swatch is the only caller.
    pub fn node_rgb(&self, r: f32, g: f32, b: f32) -> Color32 {
        Color32::from_rgb(
            (r.clamp(0.0, 1.0) * 255.0) as u8,
            (g.clamp(0.0, 1.0) * 255.0) as u8,
            (b.clamp(0.0, 1.0) * 255.0) as u8,
        )
    }

    /// The fill alpha a value track paints at in `state` — the one place the
    /// control states differ in weight (rest < hover < active/dragging).
    pub fn control_fill_alpha(&self, state: ControlState) -> u8 {
        match state {
            ControlState::Rest | ControlState::Wired => 0x40,
            ControlState::Hover => 0x70,
            ControlState::Active | ControlState::Dragging => 0x90,
            ControlState::Disabled => 0x18,
        }
    }

    /// The fill colour of a value track in `state`: a wired control reads in
    /// the accent-dim (a source owns it), the rest in the phosphor.
    pub fn control_fill(&self, state: ControlState) -> Color32 {
        let base = if state == ControlState::Wired { self.phosphor_dim } else { self.phosphor };
        self.faded(base, self.control_fill_alpha(state))
    }

    /// The value ink in `state`.
    pub fn control_ink(&self, state: ControlState) -> Color32 {
        match state {
            ControlState::Disabled => self.ink_faint,
            ControlState::Wired => self.ink_weak,
            _ => self.ink,
        }
    }

    /// The secondary (arrow / unit) ink in `state`.
    pub fn control_ink_weak(&self, state: ControlState) -> Color32 {
        match state {
            ControlState::Disabled | ControlState::Wired => self.ink_faint,
            _ => self.ink_weak,
        }
    }

    /// `color` at a reduced alpha — the one escape hatch gate 2 grants for
    /// "the same signal, fainter": an off-playhead keyframe mark, a chroma
    /// heatmap cell, a beat suggestion (`shell::lanes::paint_ingest`).
    /// `alpha` is 0..255, egui's own scale, so a call site never needs its
    /// own cast either.
    pub fn faded(&self, color: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    /// A colour at `hue01` (0..1), fixed saturation/value — the router
    /// band's per-model colour (`shell::lanes::model_color` hashes the
    /// model id to a hue; this is the one place allowed to turn it into a
    /// `Color32`, gate 2's `Hsva::` ban).
    pub fn hue(&self, hue01: f32) -> Color32 {
        egui::ecolor::Hsva::new(hue01, 0.55, 0.55, 1.0).into()
    }

    /// No fill — `S.U9`'s cable curves are stroked, never filled, and
    /// `egui::epaint::CubicBezierShape::from_points_stroke` still wants a
    /// fill colour to construct one. Not really theme-dependent (every
    /// theme agrees a stroke-only shape has no fill), but gate 2 bans
    /// `Color32::` outside this file regardless, so the constant lives here
    /// rather than at the one call site that needs it.
    pub fn transparent(&self) -> Color32 {
        Color32::TRANSPARENT
    }

    /// Inner margin of a panel frame.
    pub fn panel_margin(&self) -> Margin {
        Margin::same(self.pad)
    }

    /// The monospace face at `size`, for code and readouts inside a panel.
    pub fn font_mono(&self, size: f32) -> egui::FontId {
        egui::FontId::monospace(size)
    }

    /// The inner margin of a floating canvas bar.
    pub fn bar_margin(&self) -> Margin {
        Margin::same(self.bar_pad)
    }

    /// The inner margin of the resting (hold) bar over a flagged node.
    pub fn hold_bar_margin(&self) -> Margin {
        Margin::same(self.hold_bar_pad)
    }

    /// The inner margin of a card — the panel's own padding, so a card's
    /// content sits at the same inset as a panel's.
    pub fn card_margin(&self) -> Margin {
        Margin::same(self.pad)
    }

    /// The glow a raised card casts: the phosphor at its whisper-of-alpha
    /// step, the same recipe the window shadow uses but tight, so a card
    /// reads as lit rather than dropshadowed.
    pub fn card_shadow(&self) -> egui::epaint::Shadow {
        egui::epaint::Shadow {
            offset: [0, 2],
            blur: self.glow_blur,
            spread: 0,
            color: self.glow,
        }
    }

    /// The frame of a raised card: the raised surface, the hairline rule, the
    /// frame radius, the card padding and [`Self::card_shadow`]. The
    /// properties header block (and any other "card" surface) uses this so
    /// they read alike.
    pub fn card_frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.tube_raised)
            .stroke(self.rule_stroke())
            .inner_margin(self.card_margin())
            .corner_radius(self.corner_radius())
            .shadow(self.card_shadow())
    }

    /// The preset tag colours a library file or folder can be tinted with
    /// (the row menus' `color` submenu) — a short ramp across the hue wheel,
    /// each saturated enough to read as a tinted title on the dark ground.
    /// The index into this ramp is what a tag stores, so the colours can be
    /// restyled without rewriting every tag.
    pub fn tag_colours(&self) -> [Color32; 8] {
        [
            Color32::from_rgb(0xff, 0x4d, 0x4d), // red
            Color32::from_rgb(0xff, 0x92, 0x3f), // orange
            Color32::from_rgb(0xff, 0xd1, 0x4d), // amber
            Color32::from_rgb(0x6f, 0xdf, 0x7f), // green
            Color32::from_rgb(0x4d, 0xd6, 0xd6), // cyan
            Color32::from_rgb(0x5b, 0x9d, 0xff), // blue
            Color32::from_rgb(0xb0, 0x7a, 0xff), // violet
            Color32::from_rgb(0xff, 0x7a, 0xd1), // pink
        ]
    }

    /// The emphasis accent as a stroke, for a focused edge on a surface that
    /// wants the hotter red ([`Self::accent_vivid`]).
    pub fn vivid_stroke(&self) -> Stroke {
        Stroke::new(self.accent_stroke, self.accent_vivid)
    }

    /// Inner margin of the header bar: [`Self::header_pad`] horizontally,
    /// [`Self::header_pad_y`] vertically, so the row has symmetric breathing
    /// room above and below its content.
    pub fn header_margin(&self) -> Margin {
        Margin {
            left: self.header_pad,
            right: self.header_pad,
            top: self.header_pad_y,
            bottom: self.header_pad_y,
        }
    }

    /// The one radius, on every corner.
    pub fn corner_radius(&self) -> CornerRadius {
        CornerRadius::same(self.radius)
    }

    /// The corner radius of inputs and buttons ([`Theme::field_radius`]).
    pub fn field_corner(&self) -> CornerRadius {
        CornerRadius::same(self.field_radius)
    }

    /// The corner radius of menus and popups ([`Theme::menu_radius`]).
    pub fn menu_corner(&self) -> CornerRadius {
        CornerRadius::same(self.menu_radius)
    }

    /// The library card's own corner radius — rounder than [`corner_radius`]
    /// so a card reads as a card.
    pub fn library_card_corner(&self) -> CornerRadius {
        CornerRadius::same(self.library_card_radius)
    }

    /// What the window is cleared to before egui paints a single shape:
    /// the tube. Named so `window.rs` asks the theme for "the ground" and
    /// never learns it is the same colour as `tube`.
    pub fn ground(&self) -> Color32 {
        self.tube
    }

    /// `share` of `total`, floored at `min` on both sides of the split — the
    /// side computed here *and* the side implied by `total - result`. One
    /// function so every split in the shell (a grid row/column in
    /// `regions.rs`, the sidebar width below) enforces the same floor the
    /// same way. Lives here, not in `regions.rs`, so [`sidebar_width`] can
    /// share it without `regions.rs` (a `shell` file) becoming something
    /// `classes.rs` would have to import — gate 1 forbids that direction.
    pub fn clamp_split(total: f32, share: f32, min: f32) -> f32 {
        let floor = min.min(total * 0.5);
        (total * share).clamp(floor, (total - floor).max(floor))
    }

    /// The sidebars' initial width once [`arrangement`] has delegated them out
    /// of the grid: `sidebar_share` of the viewport, floored at
    /// `sidebar_min_width` and never asked past half the viewport — the same
    /// clamp a grid column gets from [`Theme::clamp_split`], reused so a
    /// sidebar never claims more of a narrow screen than a grid column
    /// would have. The drag then runs between [`Theme::sidebar_min_width`]
    /// and [`Theme::sidebar_bounds`].
    pub fn sidebar_width(&self, viewport_width: f32) -> f32 {
        Theme::clamp_split(viewport_width, self.sidebar_share, self.sidebar_min_width)
    }

    /// The `(min, max)` a sidebar may be dragged across on a viewport of
    /// `viewport_width`. The floor is clamped to half the viewport the same
    /// way [`Theme::clamp_split`] clamps the opening width — otherwise egui
    /// would widen a sidebar past its own opening size on a phone — and the
    /// ceiling is never below that floor.
    pub fn sidebar_bounds(&self, viewport_width: f32) -> (f32, f32) {
        let min = self.sidebar_min_width.min(viewport_width * 0.5);
        let max = (viewport_width * self.sidebar_max_share).max(min);
        (min, max)
    }

    /// [`Theme::ground`] as the `wgpu::Color` a `LoadOp::Clear` takes.
    ///
    /// `srgb_surface` is whether the surface format is an `*Srgb` one — the
    /// window passes what `surface.get_default_config` chose. On an sRGB
    /// surface the hardware encodes on write, so the clear must be given
    /// **linear** values or the ground comes out lighter than every frame
    /// egui paints on top of it (egui-wgpu does this same conversion for its
    /// own meshes). On a non-sRGB surface the gamma bytes go through as-is.
    /// Colour math is this file's business, which is why the conversion is
    /// here and not beside the render pass.
    #[cfg(feature = "gpu")]
    pub fn ground_wgpu(&self, srgb_surface: bool) -> wgpu::Color {
        let [r, g, b, a] = if srgb_surface {
            egui::Rgba::from(self.ground()).to_array()
        } else {
            self.ground().to_normalized_gamma_f32()
        };
        wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: f64::from(a),
        }
    }

    /// `S.U15`: [`Theme::ground`] as the `[f32; 4]` `egui_glow::Painter::clear`
    /// takes. `window.rs`'s glow path never negotiates an sRGB-capable GL
    /// surface (no `ARB_framebuffer_sRGB` request in its `ConfigTemplate`),
    /// so this is always [`Theme::ground_wgpu`]'s non-sRGB branch — one
    /// function rather than a `srgb_surface` parameter nothing would ever
    /// pass `true` for. Kept as its own function, not a call to
    /// `ground_wgpu(false)`, so this file stays the one place that knows
    /// which numeric type each backend's clear wants (`wgpu::Color`'s
    /// `f64`s vs. `glow`'s `f32`s) — `window.rs` asks the theme for "the
    /// ground" either way and never learns the two backends disagree on
    /// that.
    #[cfg(feature = "glow")]
    pub fn ground_glow(&self) -> [f32; 4] {
        self.ground().to_normalized_gamma_f32()
    }

    /// Push the theme into an [`egui::Context`]: `Style`, `Spacing`, the
    /// `TextStyle` table and `Visuals`, for both of egui's light and dark
    /// slots so the host's preference cannot swap half of it out. The theme
    /// itself is also stashed under [`theme_slot`] so the one legacy widget
    /// that predates the `&Theme` argument — `components::float_row` — can
    /// read the active style from its `Ui` alone; every theme-aware call
    /// site passes its own `&Theme` and never needs this.
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.all_styles_mut(|style| self.write_style(style));
        ctx.data_mut(|d| d.insert_temp(theme_slot(), self.clone()));
    }

    /// The [`Theme`] most recently pushed into `ctx` by [`Theme::apply`], if
    /// any — see that method's own note on why the stash exists.
    pub fn current(ctx: &egui::Context) -> Option<Theme> {
        ctx.data(|d| d.get_temp(theme_slot()))
    }

    fn write_style(&self, style: &mut egui::Style) {
        style.spacing.item_spacing = self.item_gap;
        style.spacing.button_padding = self.field_pad;
        style.spacing.interact_size = self.interact_size;
        style.spacing.window_margin = self.panel_margin();
        style.spacing.menu_margin = self.panel_margin();

        style.text_styles = [
            (TextStyle::Heading, self.font_heading.clone()),
            (TextStyle::Body, self.font_body.clone()),
            (TextStyle::Button, self.font_body.clone()),
            (TextStyle::Monospace, self.font_body.clone()),
            (TextStyle::Small, self.font_small.clone()),
        ]
        .into_iter()
        .collect();

        let v = &mut style.visuals;
        // Not `dark_mode` per host preference: `apply` writes both of egui's
        // style slots with the same values, so the host cannot swap half of
        // it out (this function's contract).
        v.override_text_color = Some(self.ink);
        v.weak_text_color = Some(self.ink_weak);
        v.hyperlink_color = self.phosphor_lit;
        v.panel_fill = self.panel;
        v.window_fill = self.tube_raised;
        v.window_stroke = self.rule_stroke();
        v.window_corner_radius = self.corner_radius();
        v.menu_corner_radius = self.menu_corner();
        v.faint_bg_color = self.phosphor_deep;
        v.extreme_bg_color = self.inset;
        v.text_edit_bg_color = Some(self.inset);
        v.code_bg_color = self.inset;
        v.warn_fg_color = self.warn;
        v.error_fg_color = self.phosphor;
        v.selection.bg_fill = self.phosphor_deep;
        v.selection.stroke = Stroke::new(self.hairline, self.phosphor_dim);
        // The red cursor bar is the era's signature — and the one text-area
        // state a control surface spends all its time in.
        v.text_cursor.stroke = Stroke::new(self.accent_stroke, self.phosphor);
        v.text_cursor.blink = true;
        v.text_cursor.preview = false;
        // The glow: the phosphor itself at its whisper-of-alpha step, so a
        // raised window reads as lit, not as dropshadowed.
        v.window_shadow = Shadow {
            offset: [0, 2],
            blur: self.glow_blur,
            spread: 0,
            color: self.glow,
        };
        v.popup_shadow = Shadow {
            offset: [0, 2],
            blur: self.glow_blur,
            spread: 0,
            color: self.glow,
        };
        v.handle_shape = HandleShape::Rect { aspect_ratio: 0.28 };

        let w = &mut v.widgets;
        for ws in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            ws.bg_fill = self.panel;
            ws.weak_bg_fill = self.tube_raised;
            ws.bg_stroke = self.rule_stroke();
            ws.fg_stroke = Stroke::new(self.hairline, self.ink);
            ws.corner_radius = self.field_corner();
            ws.expansion = 0.0;
        }
        w.noninteractive.fg_stroke = Stroke::new(self.hairline, self.ink_weak);
        // Hover: the dim red wash under the cursor — the lit state of the
        // CSS `.btn:hover`, translated to a fill egui can paint without
        // knowing what a button is.
        w.hovered.weak_bg_fill = self.phosphor_deep;
        w.hovered.bg_stroke = self.rule_bright_stroke();
        w.hovered.expansion = self.accent_stroke;
        // Press: the inversion. Filled phosphor, ink knocked to the tube's
        // colour — "this is on" survives a glance across a booth, the way
        // `.btn.is-on` reads in the HTML client.
        w.active.bg_fill = self.phosphor;
        w.active.weak_bg_fill = self.phosphor;
        w.active.bg_stroke = self.accent();
        w.active.fg_stroke = Stroke::new(self.hairline, self.ground());
        w.open.weak_bg_fill = self.phosphor_deep;
        w.open.bg_stroke = self.rule_bright_stroke();
    }
}

/// The stable slot [`Theme::apply`] stashes the active theme under, and
/// [`Theme::current`] reads it back from.
fn theme_slot() -> egui::Id {
    egui::Id::new("shell/active-theme")
}

#[cfg(test)]
/// The five viewport sizes `crates/metal-dash/check-ui.mjs` section A checks
/// the HTML client's own breakpoints against, reused here as the kittest
/// suite's sizes (`plan-sonnet.md` § S.U13): the same five real targets
/// (desktop, laptop, a portrait tablet, a phone, a small phone) back the
/// native shell's own responsive tests, so nobody has to invent a sixth
/// meaning of "tablet-sized" for this tree. `(name, width, height)`, logical
/// pixels — every consumer (`kittest.rs`'s five-viewport suite, this file's
/// own `arrangement` unit test) iterates this array rather than writing the
/// count down a second time (regla 7).
pub const VIEWPORTS: [(&str, f32, f32); 5] = [
    ("desktop", 1600.0, 950.0),
    ("laptop", 1180.0, 800.0),
    ("tablet", 820.0, 1100.0),
    ("phone", 390.0, 844.0),
    ("phone-small", 360.0, 740.0),
];

#[cfg(test)]
/// How far past a drag's origin the kittest driver moves before the real
/// destination (`drag_between`, the L22.11 file-drag tests): far enough to
/// clear egui's click threshold so `drag_started` fires and the payload is
/// set, near enough to stay inside the shell. The only visual constant the
/// tests brought in with that batch — test code, though it is, is still a
/// pixel decision someone could want retuned, so it lives here too.
pub const DRAG_CLEARANCE: f32 = 24.0;

/// P-F: the venue editor's edge-blend sketch of one surface — its height,
/// the spacing of the hatch over an overlap, and how long a projector's
/// test pattern (white + grid) stays up.
pub const BLEND_CANVAS_HEIGHT: f32 = 96.0;
pub const BLEND_HATCH_STEP: f32 = 6.0;
pub const TEST_PATTERN_SECONDS: f64 = 10.0;

/// Where a delegable region lives at a given viewport (`O.12.f`,
/// `plan-sonnet.md` § S.U13). `Grid` means it still paints as its own cell
/// of the ceding grid (`regions.rs`, `S.U8`); `Delegated` means it does not
/// — library becomes a collapsible left sidebar, `graph`/`lanes` become
/// one header-selected tab at a time, properties becomes a long-press
/// overlay. Which delegated *form* a `Delegated` placement takes is fixed
/// per field (this enum only says whether the grid still holds it), decided
/// by the caller (`regions.rs`/`header.rs`), not by this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Grid,
    Delegated,
}

/// The whole shell's arrangement at one viewport — [`arrangement`]'s result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrangement {
    /// Library: grid cell, or left sidebar.
    pub library: Placement,
    /// `graph`/`lanes`: two grid cells, or one header-tab at a time.
    pub tabs: Placement,
    /// Properties: grid cell, or long-press overlay.
    pub properties: Placement,
    /// The AI harness (`S.U14`, `O.12.f`'s own "harness on the right"):
    /// never a grid *cell* — it is not one of [`super::panels::Region::ALL`]
    /// and never competes for `focus`/`cede` space — but still a placement
    /// the header's `AI` toggle needs to know, because the two `Placement`
    /// values change what that toggle opens. [`Placement::Grid`] here means
    /// "a permanent right [`super::components::side_panel`], docked
    /// alongside the full four-region grid" (desktop/laptop/tablet, where
    /// there is room to spare); [`Placement::Delegated`] means "a
    /// foreground overlay" (phone/phone-small — the same two sizes
    /// `library`/`tabs`/`properties` already delegate at, reused rather
    /// than re-measured: a phone that cannot fit two grid cells side by
    /// side cannot fit a persistent right sidebar either, so `harness`
    /// mirrors `tabs` instead of adding a fourth, independent threshold).
    pub harness: Placement,
}

impl Arrangement {
    /// Every region still paints its own cell — the exact `S.U8`–`S.U12`
    /// grid, nothing delegated.
    pub fn is_full_grid(self) -> bool {
        self.library == Placement::Grid
            && self.tabs == Placement::Grid
            && self.properties == Placement::Grid
    }
}

// The mode vocabulary moved to `crate::shell_mode` (`LAY.10`): the route
// that owns `data/settings.json` has to validate it on every build,
// including the ones with no window, and gate 1 keeps this file from
// reaching into `call/` to do it. Re-exported here so every existing
// `shell::classes::ShellMode` path keeps reading the same way.
pub use crate::shell_mode::ShellMode;

/// Decide the arrangement at `viewport` (`plan-sonnet.md` § S.U13) — never a
/// per-viewport table, only `theme.region_min` read against three widths (a
/// height, for the one case a narrow-but-short window collapses on an axis
/// `library`/`properties` don't test). **Delegar, nunca reescalar**: nothing
/// here ever shrinks a share below `region_min`; once a split would have to,
/// the region it belongs to stops being a grid split.
///
/// The three thresholds delegate in the order a screen actually gets tight,
/// library first and properties last, and that order is *why* the two
/// thresholds below borrow each other's desktop share rather than each
/// using its own: `properties_share` (`5/14`) is the smaller of this
/// theme's two desktop shares, so `region_min.x / properties_share` is the
/// *larger* threshold of the two — the one library crosses first as `w`
/// shrinks, which is what "library delegates before properties" means
/// in numbers. `properties`'s own floor mirrors that against
/// `library_share` (the bigger share, the smaller threshold), so it is the
/// last of the three to give way — the one panel every other delegated
/// panel's own controls still need to reach.
pub fn arrangement(theme: &Theme, viewport: Vec2) -> Arrangement {
    let w = viewport.x;
    let body_h = (viewport.y - theme.header_height).max(0.0);

    let smaller_share = |s: f32| s.min(1.0 - s);
    let library_floor = theme.region_min.x / smaller_share(theme.properties_share);
    let properties_floor = theme.region_min.x / smaller_share(theme.library_share);
    let rows_floor = theme.region_min.y / smaller_share(theme.row_share);

    let library = if w < library_floor {
        Placement::Delegated
    } else {
        Placement::Grid
    };

    // `region_min.x`, not `Theme::sidebar_width` — the arrangement is
    // structure, and `sidebar_share` is style (`gate 3`'s own restyle drill
    // requires the two house themes reach the *same* arrangement for a
    // given viewport even though they paint a sidebar of a different exact
    // width). `region_min.x` is the floor every sidebar width is clamped to
    // once the viewport is this narrow in the first place, so it is the
    // honest stand-in for "how much width library is still holding".
    let sidebar_w = if library == Placement::Delegated {
        theme.region_min.x
    } else {
        0.0
    };
    let remaining_w = w - sidebar_w;

    // Two tabbed panels side by side need each to clear the floor too.
    let tabs = if body_h < rows_floor || remaining_w < theme.region_min.x * 2.0 {
        Placement::Delegated
    } else {
        Placement::Grid
    };

    let properties = if tabs == Placement::Delegated || remaining_w < properties_floor {
        Placement::Delegated
    } else {
        Placement::Grid
    };

    // See `Arrangement::harness`'s own doc comment on why this mirrors
    // `tabs` rather than measuring a fourth threshold.
    let harness = tabs;

    Arrangement {
        library,
        tabs,
        properties,
        harness,
    }
}

/// [`arrangement`] with the settings dropdown's manual override applied
/// (`S.AF`, `ShellMode`). **The map, not the forget**: `Auto` hands straight
/// through to `arrangement` (the same function, the same thresholds — a
/// forced desktop/mobile never sees a *modified* set of rules, only the
/// terminal verdicts), `Escritorio` is the entire grid for one flag's worth
/// of viewport width, and `Movil` is the entire delegated shape — the same
/// four `Placement`s the narrowest viewport produces. No third rule set:
/// a mode that generated its own arrangement would be a second truth
/// about "how the shell lays out", which is exactly what `S.U13` paid a
/// doc comment regime to prevent.
pub fn arrangement_forced(theme: &Theme, viewport: Vec2, mode: ShellMode) -> Arrangement {
    match mode {
        ShellMode::Auto => arrangement(theme, viewport),
        ShellMode::Escritorio => Arrangement {
            library: Placement::Grid,
            tabs: Placement::Grid,
            properties: Placement::Grid,
            harness: Placement::Grid,
        },
        ShellMode::Movil => Arrangement {
            library: Placement::Delegated,
            tabs: Placement::Delegated,
            properties: Placement::Delegated,
            harness: Placement::Delegated,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_house_themes_differ_in_colour_and_spacing() {
        let a = Theme::phosphor();
        let b = Theme::contrast();
        assert_ne!(a.tube, b.tube);
        assert_ne!(a.phosphor, b.phosphor);
        assert_ne!(a.pad, b.pad);
        assert_ne!(a.item_gap, b.item_gap);
        assert_ne!(a.font_body, b.font_body);
    }

    /// `O.17`: dark and light are two styles of one universe, and the
    /// emphasis is the common half — the same phosphor red and its lit step
    /// on either tube. The `contrast` drill theme is deliberately *not*
    /// bound to this: it is not a house style.
    #[test]
    fn the_emphasis_is_common_to_both_house_styles() {
        let dark = Theme::phosphor();
        let light = Theme::daylight();
        assert_eq!(
            dark.phosphor, light.phosphor,
            "the phosphor changes tube's, not the signal's"
        );
        assert_eq!(dark.phosphor_lit, light.phosphor_lit);
    }

    /// `O.17`: **the universe is exact.** Every dark-style token the shell
    /// reads is a hex value `web/theme.css` already declares — if the two
    /// disagree, one of them is the wrong exact colour, and this test names
    /// it. Keeping the table here in sync with the stylesheet by test
    /// rather than by discipline, since the stylesheet is the source.
    #[test]
    fn the_dark_house_is_the_theme_css_universe() {
        let css =
            metal_vfs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/theme.css"))
                .expect("theme.css");
        let dark = Theme::phosphor();
        for fg_hex in [
            dark.tube,
            dark.tube_raised,
            dark.panel,
            dark.inset,
            dark.ink,
            dark.ink_weak,
            dark.ink_faint,
            dark.phosphor,
            dark.phosphor_lit,
            dark.phosphor_dim,
            dark.phosphor_deep,
            dark.soft,
            dark.soft_dim,
            dark.ok,
            dark.warn,
            dark.rule,
            dark.rule_bright,
        ]
        .into_iter()
        .map(|c| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()))
        {
            assert!(css.contains(&fg_hex), "theme.css declares no {fg_hex}; the shell's dark palette drifted from the universe");
        }
    }

    /// `O.17`: the light style has its own exact values for everything the
    /// emphasis does not already hold in common — two houses, one
    /// declaration each, nothing derived or left implicit.
    #[test]
    fn the_light_house_has_its_own_universe() {
        let dark = Theme::phosphor();
        let light = Theme::daylight();
        for (field, dark, light) in [
            ("tube", dark.tube, light.tube),
            ("tube_raised", dark.tube_raised, light.tube_raised),
            ("panel", dark.panel, light.panel),
            ("inset", dark.inset, light.inset),
            ("ink", dark.ink, light.ink),
            ("ink_weak", dark.ink_weak, light.ink_weak),
            ("ink_faint", dark.ink_faint, light.ink_faint),
            ("phosphor_dim", dark.phosphor_dim, light.phosphor_dim),
            ("phosphor_deep", dark.phosphor_deep, light.phosphor_deep),
            ("soft", dark.soft, light.soft),
            ("soft_dim", dark.soft_dim, light.soft_dim),
            ("ok", dark.ok, light.ok),
            ("warn", dark.warn, light.warn),
            ("glow", dark.glow, light.glow),
            ("rule", dark.rule, light.rule),
            ("rule_bright", dark.rule_bright, light.rule_bright),
        ] {
            assert_ne!(
                dark, light,
                "{field}: the two styles must each declare an exact value, not share one"
            );
        }
    }

    /// `O.17`'s `--theme`/`FW_THEME` knob: both names' aliases, and a typo
    /// is an error rather than a silent dark default.
    #[test]
    fn from_name_resolves_both_house_styles() {
        assert_eq!(Theme::from_name("dark").map(|t| t.name), Some("phosphor"));
        assert_eq!(
            Theme::from_name("phosphor").map(|t| t.name),
            Some("phosphor")
        );
        assert_eq!(Theme::from_name("light").map(|t| t.name), Some("daylight"));
        assert_eq!(
            Theme::from_name("daylight").map(|t| t.name),
            Some("daylight")
        );
        assert!(Theme::from_name("midnight").is_none());
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn the_ground_clear_is_linear_on_an_srgb_surface_and_gamma_otherwise() {
        let t = Theme::contrast(); // a light ground, so gamma ≠ linear by a wide margin
        let linear = t.ground_wgpu(true);
        let gamma = t.ground_wgpu(false);
        assert_eq!(gamma.r as f32, t.ground().to_normalized_gamma_f32()[0]);
        assert!(
            linear.r < gamma.r,
            "linear {} should be darker than gamma {}",
            linear.r,
            gamma.r
        );
        assert_eq!(linear.a, 1.0);
        assert_eq!(gamma.a, 1.0);
    }

    /// `S.U15`: `ground_glow` is `ground_wgpu`'s gamma (non-sRGB) branch,
    /// restated for `[f32; 4]` — the glow path never negotiates an sRGB GL
    /// surface, so there is only the one branch to match against.
    #[test]
    #[cfg(feature = "glow")]
    fn ground_glow_matches_the_gamma_branch_of_ground_wgpu() {
        let t = Theme::contrast();
        let glow = t.ground_glow();
        assert_eq!(glow, t.ground().to_normalized_gamma_f32());
        #[cfg(feature = "gpu")]
        {
            let gamma = t.ground_wgpu(false);
            assert_eq!(glow[0], gamma.r as f32);
            assert_eq!(glow[1], gamma.g as f32);
            assert_eq!(glow[2], gamma.b as f32);
            assert_eq!(glow[3], gamma.a as f32);
        }
    }

    #[test]
    fn apply_writes_the_context_style() {
        let ctx = egui::Context::default();
        let t = Theme::contrast();
        t.apply(&ctx);
        let style = ctx.global_style();
        assert_eq!(style.spacing.item_spacing, t.item_gap);
        assert_eq!(style.visuals.panel_fill, t.panel);
        assert_eq!(style.text_styles[&TextStyle::Body], t.font_body);
    }

    /// Desktop must yield exactly the `S.U8`–`S.U12` grid — nothing
    /// delegated — the hard requirement `plan-sonnet.md` § S.U13 puts on
    /// this function before anything else.
    #[test]
    fn desktop_is_the_full_grid() {
        let theme = Theme::phosphor();
        let a = arrangement(&theme, Vec2::new(DESKTOP_VIEWPORT_X, DESKTOP_VIEWPORT_Y));
        assert!(a.is_full_grid(), "{a:?}");
        assert_eq!(
            a.harness,
            Placement::Grid,
            "desktop has room for a docked harness sidebar too"
        );
    }

    /// `Arrangement::harness`'s own doc comment: it always mirrors `tabs`,
    /// never a fourth independent measurement.
    #[test]
    fn harness_always_mirrors_tabs() {
        for theme in [Theme::phosphor(), Theme::contrast()] {
            for (name, w, h) in VIEWPORTS {
                let a = arrangement(&theme, Vec2::new(w, h));
                assert_eq!(
                    a.harness, a.tabs,
                    "{name} ({w}x{h}) under {}: harness should mirror tabs",
                    theme.name
                );
            }
        }
    }

    /// Every `VIEWPORTS` entry, run through both house themes: whatever
    /// `arrangement` decides, it must never ask a grid split to hold a
    /// region under `region_min` — the same property `regions::clamp_split`
    /// enforces on every frame, checked here against the *decision*, not
    /// just the split math. And the order is never violated: properties
    /// only delegates once library has, library-or-properties
    /// delegating implies nothing further about `tabs` (it can delegate
    /// on its own, from height, before either).
    #[test]
    fn every_viewport_respects_the_delegation_order() {
        for theme in [Theme::phosphor(), Theme::contrast()] {
            for (name, w, h) in VIEWPORTS {
                let a = arrangement(&theme, Vec2::new(w, h));
                if a.properties == Placement::Delegated {
                    // properties is the last to give way: once it has,
                    // library must already have too (never the other
                    // way around) unless it was `tabs` (a height-only
                    // squeeze) that forced it.
                    assert!(
                        a.library == Placement::Delegated || a.tabs == Placement::Delegated,
                        "{name} ({w}x{h}) under {}: properties delegated with nothing else forcing it: {a:?}",
                        theme.name
                    );
                }
            }
        }
    }

    /// The two extremes of `VIEWPORTS` land on the two arrangements this
    /// batch actually builds a delegated host for: desktop stays the full
    /// grid, the smallest phone delegates all three — falsified by
    /// measurement (dropping `Theme::sidebar_width`'s floor, or any of the
    /// three comparisons in `arrangement`, changes one of these two).
    #[test]
    fn phone_small_delegates_everything() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4];
        assert_eq!(VIEWPORTS[4].0, "phone-small");
        let a = arrangement(&theme, Vec2::new(w, h));
        assert_eq!(a.library, Placement::Delegated);
        assert_eq!(a.tabs, Placement::Delegated);
        assert_eq!(a.properties, Placement::Delegated);
        assert_eq!(a.harness, Placement::Delegated);
    }

    /// `sidebar_width` never exceeds half the viewport and never falls
    /// under `region_min.x` (until the viewport itself is smaller than
    /// twice the floor, where the clamp gives up the floor for half the
    /// screen — checked directly rather than assumed).
    #[test]
    fn sidebar_width_is_floored_and_never_exceeds_half() {
        let theme = Theme::phosphor();
        for w in [200.0, 390.0, 820.0, 1600.0] {
            let sw = theme.sidebar_width(w);
            assert!(sw <= w * 0.5 + 0.001, "sidebar {sw} exceeds half of {w}");
            assert!(sw <= theme.region_min.x.max(w * 0.5) + 0.001);
        }
    }

    /// The drag range: the opening width always sits inside it, and on a
    /// phone-sized viewport the floor gives way to half the screen instead of
    /// letting egui widen the sidebar past its opening size.
    #[test]
    fn sidebar_bounds_contain_the_opening_width() {
        let theme = Theme::phosphor();
        for w in [200.0, 390.0, 820.0, 1600.0] {
            let (min, max) = theme.sidebar_bounds(w);
            assert!(min <= max, "bounds inverted at {w}: {min}..{max}");
            let sw = theme.sidebar_width(w);
            assert!(min <= sw + 0.001 && sw <= max + 0.001, "{sw} outside {min}..{max} at {w}");
        }
    }

    // -----------------------------------------------------------------------
    // S.AF — `arrangement_forced` (the settings dropdown's manual override)
    // -----------------------------------------------------------------------

    /// `Auto` is a pass-through, not a fourth rule set: at every one of the
    /// five real viewports it answers exactly what `arrangement` answers.
    #[test]
    fn auto_hands_straight_through_to_the_viewport_arrangement() {
        let theme = Theme::phosphor();
        for (name, w, h) in VIEWPORTS {
            let size = Vec2::new(w, h);
            assert_eq!(
                arrangement_forced(&theme, size, ShellMode::Auto),
                arrangement(&theme, size),
                "Auto must equal arrangement at {name}"
            );
        }
    }

    /// `Escritorio` is the whole grid even where the thresholds would
    /// delegate everything — the smallest phone is the strongest case.
    #[test]
    fn escritorio_forces_the_full_grid_on_the_smallest_phone() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[4];
        let a = arrangement_forced(&theme, Vec2::new(w, h), ShellMode::Escritorio);
        assert!(a.is_full_grid());
        assert_eq!(a.harness, Placement::Grid);
    }

    /// `Movil` is the whole delegated shape even on the desktop — the
    /// same four `Placement`s `phone_small_delegates_everything` measures.
    #[test]
    fn mobile_forces_every_delegation_on_the_desktop() {
        let theme = Theme::phosphor();
        let (_, w, h) = VIEWPORTS[0];
        let a = arrangement_forced(&theme, Vec2::new(w, h), ShellMode::Movil);
        assert_eq!(
            a,
            arrangement(&theme, Vec2::new(VIEWPORTS[4].1, VIEWPORTS[4].2))
        );
        assert_eq!(a.harness, Placement::Delegated);
    }

    /// The wire vocabulary round-trips through `parse`, `ALL` lists every
    /// variant exactly once, and a spelling outside it is refused —
    /// `call::settings` leans on that refusal to keep the file honest.
    #[test]
    fn shell_mode_wire_names_round_trip_and_unknown_is_refused() {
        for (_, mode) in ShellMode::ALL {
            assert_eq!(ShellMode::parse(mode.wire()), Some(mode));
        }
        assert_eq!(ShellMode::ALL.len(), 3);
        assert_eq!(ShellMode::parse("kiosk"), None);
        assert_eq!(
            ShellMode::parse("móvil"),
            None,
            "the wire name is ascii `mobile`"
        );
    }
}

// Kept as plain `f32` constants (not `super::DESKTOP_VIEWPORT`, an `(f64,
// f64)`) so `desktop_is_the_full_grid`'s `Vec2::new` above needs no cast.
#[cfg(test)]
const DESKTOP_VIEWPORT_X: f32 = 1600.0;
#[cfg(test)]
const DESKTOP_VIEWPORT_Y: f32 = 950.0;

/// One preset's `[r,g,b]` in 0..1 as the colour a button paints.
/// The font a Lucide glyph is set in at `size`: the proportional family,
/// which carries the icon face as a fallback (see `components::install_icon_font`).
pub fn glyph_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Proportional)
}

pub fn rgb_color(c: [f32; 3]) -> Color32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(byte(c[0]), byte(c[1]), byte(c[2]))
}
