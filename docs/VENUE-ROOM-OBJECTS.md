# Venue: empty scene and room objects

Two changes to the venue editor in `metal-dash`.

## 1. An empty room is a scene

A venue with no document no longer shows a disclaimer. The canvas paints an
empty scene: the ground and its grid, nothing else.

- `crates/metal-dash/src/shell/panels/venue_canvas.rs`
  - `empty_venue() -> Venue` — a hand-written default `Venue` (`Meta` with empty
    id and name, `Indoor` setting, every collection empty).
  - `paint_room_absent` calls `paint_empty_room(ui, theme, rect, scene)`, which
    paints `empty_venue()` through `viewport::paint_room` — the same painter the
    real room uses. The camera is fitted once with `OrbitCamera::room`. The bar
    and the gallery still paint over it.
- Test: `a_room_with_no_document_paints_an_empty_scene`.

## 2. The objects tab

The gallery popup (`venue_popup.rs`) has a new **Objects** tab between
*Projections* and *Fixtures*. It adds furniture and stage pieces to the room from
presets, and lists what the room holds with a delete on each row.

Layout, top to bottom:

1. A **name** field (`State.name`) — optional label for the next object.
2. **In the room** — one row per object (`id`, `kind`, trash button). When the
   room is empty: `— nothing in the room yet —`. The list sits above the cards so
   it is never clipped by the card scroll band.
3. The **preset cards**, each with its size in metres and a drawing of the box.

### Presets

`OBJECTS: &[ObjectPreset]` in `venue_popup.rs`. Labels are declared with
`tr_key(...)` and painted raw (`preset.label`), so the i18n scanners see them.

| id | label | kind | route (`op`) | size m (w × h × d) | at m (x, y, z) |
|---|---|---|---|---|---|
| `stage` | stage platform | stage | `stage` | 4.0 × 0.4 × 3.0 | 0.0, 0.2, -2.0 |
| `riser` | drum riser | stage | `stage` | 2.0 × 0.3 × 1.5 | 0.0, 0.15, -2.8 |
| `bar` | bar counter | box | `object` | 4.0 × 1.1 × 0.6 | 5.0, 0.55, 0.0 |
| `truss` | lighting truss | truss | `truss` | 6.0 × 0.3 × 0.3 | 0.0, 4.0, -2.0 |
| `wall` | side wall | box | `object` | 0.2 × 3.0 × 6.0 | -4.5, 1.5, -1.0 |
| `lectern` | lectern | box | `object` | 0.6 × 1.1 × 0.5 | 0.0, 0.55, -1.2 |
| `dummy` | figure | dummy | `dummy` | 0.44 × 1.8 × 0.44 | 1.0, 0.9, 0.0 |

The `at` and `size_m` values are the defaults the card posts. Each id is made
unique in the room by `next_object_id` (`"{preset.id}-{n}"`, first free `n`).

### Two routes to the room

`add_object` and `delete_object` post to `POST /venue`:

- **Generic objects** — `op: "object"`. Body:
  `{op, id, object, name, kind, form: "box", at, size, yaw}`.
  Delete is the same route with `delete: true`: `{op: "object", id, object, delete: true}`.
- **Stage, truss, dummy** — `op` is the preset's own op (`"stage"`, `"truss"`,
  `"dummy"`). Body: `{op, id, object, name, at, size, yaw}`.

The room's list reads back `GET /venue` → `venue.object` (serde rename in
`crates/metal-perform/src/venue.rs`). The list is therefore what the scene holds,
never a local copy.

### Translations

`crates/metal-dash/locales/es/venue.toml` holds the Spanish strings for the tab,
the presets and the list. `stage`, `truss`, `person` and `objects` were not used as
keys: `locales/es/properties.toml` already owns `"stage" = "etapa"`, and a second
owner would clash.

## Tests

- `shell::kittest::tests::the_objects_tab_posts_and_removes_a_room_object` — click
  a card, check the POST body; then click the row's `remove bar-1 from the room`
  and check the delete POST.
- `a_room_with_no_document_paints_an_empty_scene` — the default state.

Run with `--test-threads=1`. `cargo fmt -p metal-dash` reformats the whole crate
and must not be run; check formatting with `cargo fmt -- --check` on touched
files only.
