# add-fixture — add a fixture profile without widening the schema

A checklist for adding a light to the rig as **data** (`data/fixtures/<model>.toml`),
and for the few cases where the profile cannot express what the light does and
the *row schema* has to grow. The living examples are `cabeza-7r-230` (DMX moving
head), `godox-sl60iibi` (Bluetooth, temperature lamp) and `moving-head-beam`
(DMX, beam shaping). Read `wire-new-model.md` first if you have not: it explains
why the schema is generated and what breaks when it is stale.

## The rule this file exists to protect

**A fixture is a reader of the row, never a row.** A profile's `[[channel]]`s say
*which* existing `sig_*` float reaches *which* DMX slot, and over what range
(`crates/metal-perform/src/fixture.rs:134`). Adding a fixture — even a whole new
class of fixture — must fit inside `metal_schema::SIGNAL_FLOATS`
(`crates/metal-schema/src/lib.rs:2030`) and change **no** width, **no** column
index and **no** saved row. That is what "neutralize it via prompt" means here:
the prompt for a fixture addition is this file, and following it leaves the
schema untouched.

The schema only grows when the *room's vocabulary* grows — when a light does
something no existing float can say. That is a separate, older decision
(BLE Phase B, 2026-09-27) and §6 below is the only part of this file that
touches it.

---

## 1. The map before touching anything

| What | Where | Authoritative? |
| --- | --- | --- |
| The 13 signal floats, in order | `crates/metal-schema/src/lib.rs:2030` `SIGNAL_FLOATS` | **yes** |
| Base/saved rows, located from the end | `read_signal` (`lib.rs:2290`) — `len() - EFFECTS_WIDTH - SIGNAL_WIDTH` | derived |
| Profiles | `data/fixtures/*.toml` | **yes** |
| The reader/validator of a profile | `crates/metal-perform/src/fixture.rs` (`parse` at 655, `load` at 728) | — |
| The patch (which light sits at which address) | `data/agent/venues/<room>.toml` (`[[fixture]]`) | **yes** |
| The float manifest | `data/schema.json` — **generated** | derived |
| Sync allowlist | `crates/metal-config/src/lib.rs` `allowseed_default` | **yes** |

`SIGNAL_FLOATS` today (index → name → range → default):

```
0  sig_level  0..1       0.0      master / dimmer
1  sig_hue    0..1       0.0      colour
2  sig_sat    0..1       1.0      colour
3  sig_strobe 0..24      0.0      hertz at top of range
4  sig_pan   -1..1       0.0      signed, 0 = straight ahead
5  sig_tilt  -1..1       0.0
6  sig_a      0..1       0.0      unnamed spare (gobo/prism/amber/…)
7  sig_b      0..1       0.0      unnamed spare
8  sig_cct    1800..10000 5600.0  kelvin (BLE Phase B)
9  sig_zoom   0..1       0.0      beam shaping (BLE Phase B)
10 sig_focus  0..1       0.0
11 sig_iris   0..1       0.0
12 sig_frost  0..1       0.0
```

`Selector` (`fixture.rs:35`) can read all 13 by name (`sig_` prefix optional).
`selectors()` (`fixture.rs:588`) reports, in canonical order, the ones a profile
actually declares — that list is what the fixture page lays out.

## 2. Write the profile — `data/fixtures/<model>.toml`

The file stem **is** the model id. Every field except `[[channel]]` has a
default; only what the head actually is needs writing.

```toml
# <MODEL> — one line: what it is and where it came from.
#
# Source: <the desk profile / GDTF / the manual page>. Slots are 0-based
# offsets from `address` (1-based, the way a desk numbers them).

label = "<Human Name>"          # defaults to the file stem
universe = 0                    # meaningless for Bluetooth
address = 1                     # 1-based start; a room row may override

[limits]                        # the safety/geometry envelope (all defaulted)
pan = [-270.0, 270.0]
tilt = [-135.0, 135.0]
speed = 120.0                   # degrees/second, the slew model
rotation = [0.0, 360.0]
zoom = [4.0, 60.0]
dimmer = [0.0, 1.0]
cct = [1800.0, 10000.0]
max_dimmer = 1.0                # cap a light that is too bright for the room
no_strobe = false               # refuse strobe before the wire
# [[limits.no_go]]              # a pan/tilt rectangle the head must not enter
# pan = [..]; tilt = [..]

[[channel]]
signal = "pan"                  # level|hue|sat|strobe|pan|tilt|a|b|cct|zoom|focus|iris|frost
coarse = 0                      # relative slot of the high byte
fine = 1                        # optional: 16-bit channel
range = [-0.5, 0.5]             # the slice of the float's units that is full travel
physical = [0.0, 65535.0]       # raw slot range; default 0..255 / 0..65535 with `fine`
```

Rules enforced by `parse` (`fixture.rs:655`) — a file that breaks one is
**skipped with a line on stderr**, never a crash, and therefore silently drives
nothing:

- at least one `[[channel]]`;
- a signal may not repeat;
- a slot may not be written twice (coarse or fine);
- `coarse` and `fine` may not be the same slot;
- a channel's `range` may not be empty;
- **DMX** profile: *every* channel needs `coarse`;
- **Bluetooth** profile (`wire = "bluetooth"`): needs a `[bluetooth]` table and
  must **not** carry `coarse`/`fine` on any channel.

Bluetooth profile (no slots at all — the lamp is addressed by profile name):

```toml
label = "Godox SL60II Bi"
wire = "bluetooth"

[bluetooth]
protocol = "godox-mesh"

[[channel]]
signal = "level"

[[channel]]
signal = "cct"
range = [2800.0, 6500.0]        # the lamp's own window; `light_state` clamps into it
```

### Choosing the selector without inventing one

Look at what the head physically does, then pick the closest existing float.
Prefer a **named** float over `a`/`b`; `a`/`b` are deliberately unnamed and exist
for things no shared list should claim to name.

| The head has… | Use | Notes |
| --- | --- | --- |
| R/G/B, a colour wheel, amber/UV | `hue` + `sat` (+ `level`) | colour arrives as HSV; `a`/`b` for warm/cool white |
| separate dimmer | `level` | a par with no dimmer slot folds level into its colour channels — say so in the header comment |
| temperature | `cct` | always declare the lamp's own `range`, not the float's |
| zoom / focus / iris / frost | the float of the same name | these four are `patch_owned` (`fixture.rs:165`): `PatchMode::Colour` holds them, `PatchMode::Patch` drives them |
| gobo, prism, rotation, mode, colour macro | `a` or `b` | one per head; reuse `rotation`/`zoom` in `[limits]` only for geometry |
| a smoke/fog machine | `level` only | the venue has no haze float; see `hurricane-1600` |
| a 2-D LED matrix | — | not a profile concept: `kind = "matrix"` + `[fixture.matrix]` in the room row |

If nothing on that table is honest — the light needs a float that does not
exist — **stop** and go to §6. Do not overload a float that already means
something else on the same profile, and do not add a second channel for one
signal (refused).

## 3. Patch it in the room — `data/agent/venues/<room>.toml`

The profile is the catalogue entry; the room row is the physical unit.

```toml
[[fixture]]
id = "head-1"                   # unique in the room; a venue.fixture node points at this
name = "head 1"
kind = "moving"                 # wash|spot|led_strip|moving|backlight|panel|continuous|led_bar|camera|matrix
profile = "<model>"             # the file stem from §2
at = [-4.2, 8.75, -1.2]         # metres, +y up, floor y = 0
yaw = 0.0
pitch = -58.0
beam = 42.0                     # degrees
wire = "dmx"                    # dmx|sacn|usb|bluetooth
drive = "fixed"                 # fixed (operator values) | frame (sampled from the projection)
universe = 1                    # optional: overrides the profile
address = 1                     # optional: overrides the profile; 1..512
group = "stage-wash"            # a group a venue.fixtures node can drive alone
notes = "..."                   # where it came from, desk survey, quirks
```

- **Address span is the profile's, not a number you type.** `heads_of`
  (`wire.rs:968`) takes `max(coarse, fine) + 1` as the span. Two rows on the
  same wire+universe whose windows touch are an overlap the venue editor flags.
- A row with `wire` outside `dmx`/`sacn` (Bluetooth, USB) is **skipped** by the
  sACN/bus walk — it is driven through an `out.bluetooth`/`out.serial` node
  instead. A Bluetooth row's `wire` must agree with the profile's.
- Nothing is invented: the universe is the one already in the room.
- `fiesta-resort.toml` patches **62** units across 13 profiles; every one is
  `wire = "dmx"`, `drive = "fixed"`. If your new row changes either, that is a
  show decision, not a fixture one — note it in the row's `notes`.

## 4. The gates (all of them, no exceptions)

```
cargo test -p metal-perform           # profiles parse; the directory-wide colour test
cargo test -p metal-schema            # the signal block's width/anchors
cargo test -p metal-render            # the manifest gate (schema.json is derived, not touched)
cargo test -p metal-graph -p metal-agent
cargo test -p metal-dash              # i18n literal gates; may carry unrelated WIP in red
```

Two `metal-perform` tests walk `data/fixtures/` from disk and will fail in the
way a new profile usually fails first:

- `all_four_phase_b_profiles_parse` (`wire.rs`) — every file parses;
- `a_colour_reaches_every_profile_through_its_own_channels`
  (`fixture.rs:1394`) — a red and a blue colour through **your** channels: a
  level/hue byte must equal what `Channel::raw` says, a `cct` channel must read
  warmer for red than blue, and a warm/cool `a`/`b` pair must split the same
  way. If your profile has a `cct` channel it must reach `light_state`; if it
  has `hue`, a saturated colour must ask for HSI.

**Do not touch `data/schema.json`.** It is emitted from the renderers, and
adding a fixture does not add a model. `metal-render/tests/schema.rs` fails if
you hand-edit it or forget `cargo run -p metal-render --bin emit-schema` after a
real model change.

**Do not add anything to the sync allowlist.** `fixtures/*.toml` is already
there (`metal-config/src/lib.rs` `allowseed_default`), so a new profile crosses
the wire to every paired node with no config edit. If you find yourself editing
`allowseed`, you are moving a file out of `data/fixtures/` — don't.

## 5. Use it

Ratings of effort, smallest first:

1. **The room row alone.** A patch entry is enough for an `out.*` node and for
   `venue.fixtures` (the group node) — the picture is sampled where the light
   lands.
2. **A `venue.fixture` node** when the light must take its colour from a model,
   a `sig_*` row, or the frame, and be aimed by a row:
   `POST /graph/node {op:"add", kind:"venue.fixture", config}` — `venue`,
   `fixture` (the row id), `colour_from` = `frame` | `model` | `signal`,
   `drive` = `colour` | `patch`, plus `spread`/`pick`/`offset`/`hue_lock`
   (`wire.rs:1232`). The node is a pointer: it copies no address or profile.
3. **A switch/overlay** on it: `add_switch` with `kind = "fixture"` and the row
   id; blackout reaches it through `out.*`'s `forced` (`fixture.rs:849`).

To aim a head **from the row's `sig_pan`/`sig_tilt`** (the hotel-fiesta rule),
the node must be `drive = "patch"` (`PatchMode::Patch`, `wire.rs:1258`); with
`colour` — the default, and safe for every pre-existing venue — pan/tilt and
the four beam-shaping channels are packed at their zero travel, so the head
lights without moving.

For a DMX profile that targets a real desk, pan/tilt `range` is the head's
declared travel from `[limits]`, *not* `[-0.5, 0.5]`. The example profiles carry
the narrower `range` because they were authored against a simulation that was
never slewed; a production head must map the full `-1..1` to full `[limits]`
travel or it will never leave the centre. (The `fixture.rs` unit-test fixture
`MOVING_HEAD` keeps `[-0.5, 0.5]` deliberately — do not "fix" it.)

## 6. When the schema really has to grow (the part that is not neutral)

Read this twice before doing it. **Everything above is reversible; this is not.**

### 6.1 Decide it is needed

A new float is justified only when:

- the thing is *not* a pixel and cannot be read out of the frame (that argument
  is `SIGNAL_FLOATS`'s own rationale, `lib.rs:2005`);
- no existing float can carry it without lying (see §2's table);
- it is a property of **the room**, not of one model or one fixture — a profile
  reads it, it does not define it.

If it is one head's oddity, use `a`/`b` and document it in the profile header.
If it is one model's look, it belongs in that model's own floats, not here.

### 6.2 Append, never insert

New floats go at the **end of `SIGNAL_FLOATS`** (`lib.rs:2030`). The signal
block sits *before* the effects block, and half the workspace locates the
effects with `len() - EFFECTS_WIDTH` and the signal with
`len() - EFFECTS_WIDTH - SIGNAL_WIDTH`. Inserting in the middle keeps every read
in range and shows the wrong picture; that is the failure this whole list's doc
comment warns about (`lib.rs:2019`).

### 6.3 Make old rows migrate

A saved row written before the append is short by the new floats. That is what
the two "width before" anchors are for:

- `SIGNAL_WIDTH_BEFORE_BLE = 8` (`lib.rs:2103`) — rows from before `sig_cct`…`sig_frost`;
- `SIGNAL_WIDTH_BEFORE_COLOUR = 13` (`lib.rs:2110`) — rows from before `col_*`.

Add a third anchor (`SIGNAL_WIDTH_BEFORE_<THING> = <the width before this
append>`) and register it in **both** gap lists:

- `migrate_signal_row` (`lib.rs:2129`) — the `[SIGNAL_WIDTH_BEFORE_BLE,
  SIGNAL_WIDTH_BEFORE_COLOUR]` array;
- `migrate_effects_row` (`lib.rs:2162`) — its `signal_gaps` array.

Both are "only act on a length I can account for exactly" functions: get this
wrong and a row is either left short (driven at defaults, silently) or padded in
the wrong place (correct width, wrong picture). If the new float belongs in the
*effects* tail instead, it goes through `LATE_ROW_GROUPS` / `row_tail_floats`
(`lib.rs:1797`) — same idea, different anchor.

### 6.4 The knock-on list (what "adding the floats also touched" means)

These all move or need a decision the moment `SIGNAL_WIDTH` changes:

| What | Where | What to do |
| --- | --- | --- |
| `read_signal` | `lib.rs:2290` | add the column at its index |
| `Signal` struct | `lib.rs:2314` | add the field |
| Every `Signal { .. }` literal on the render/wire path | `metal-perform/src/fixture.rs:476, 787, 896, 1067`; `metal-perform/src/wire.rs:2858`; `metal-dash/src/shell/panels/blackout.rs:238, 290`; `metal-perform/src/gdtf.rs:621`; and the test helpers beside them | add the field; prefer `SIGNAL_FLOATS[i].default` over a magic number |
| `Selector` enum + `read`/`*_range`/`name`/`parse` | `fixture.rs:35–128` | append the variant last (the enum is `Serialize`; a saved `"a"` must still round-trip) |
| `selectors()` canonical order | `fixture.rs:588` | append so the fixture page's column order is stable |
| `patch_owned()` | `fixture.rs:165` | only if the new float moves/shapes the beam |
| `Colour`/`Patch` packing | `frame_mode` `fixture.rs:469` | decide what `held` holds for the new channel |
| Bluetooth state | `light_state` `fixture.rs:548` | only if a lamp reads the float; `metal_ble::LightState` (`metal-ble/src/lib.rs:28`) may need a field and every `metal-ble` writer/test updated |
| Frame path | `frame_signal` (`wire.rs:747`) → `colour_signal` (`fixture.rs:1063`) | give the new float an authored value, not a borrowed one |
| Fallback | `fallback_signals` (`fixture.rs:769`) | keep the new float at its schema default (the comment there explains why the signature does not grow) |
| Renderer tail read | `metal-render/src/model.rs:101` (`camera_track_offset`) uses `SIGNAL_WIDTH` | moves by itself, but re-run the render gate |
| Timeline "eight floats" doc | `metal-dash/src/author/models/timeline.rs:71` | the prose lags; fix it if you are in there |
| `data/schema.json` | — | **no change** from a signal append; the block is shared, not per-model |
| Golden frames | `metal-perform/src/engine.rs` `GOLDEN` | the signal block sits before the effects, so a widening re-composes `empty-1`; re-freeze with `FW_PRINT_GOLDEN=1` and say so in the commit (precedent: BLE Phase B there) |

Also check any consumer that hard-codes a column instead of a name. The known
ones read names: `metal-graph/src/params.rs:536` filters the shared block by
name, `metal-perform/tests/templates.rs:34` builds its shared set from
`SIGNAL_FLOATS`, and `param_aliases.rs` carries no `sig_*` entry (verify rather
than assume). Everything else that is index-based should become name-based the
day you touch it.

### 6.5 Document and regenerate

- `data/fixtures/*.toml` gains the `[[channel]]`s — **no** regeneration, because
  fixtures are data.
- The commit message must say the widening is deliberate and name the
  `SIGNAL_WIDTH_BEFORE_*` anchor it added. The BLE Phase B commit is the
  template.
- Only if a *model* was added or reordered do you run
  `cargo run -p metal-render --bin emit-schema` (that is `wire-new-model.md`'s
  job, not this file's).

## 7. Gotchas

1. **A malformed profile is a skip, never a crash** (`fixture.rs:1`). The show
   starts; the light simply never moves and nothing on air says why. After
   writing a profile, run `cargo test -p metal-perform` (it walks
   `data/fixtures/` from disk) — do not rely on the rig to tell you.
2. **A file that parses but is wrong** is worse than one that does not: a wrong
   `range` compresses or clamps travel silently; a wrong `coarse` writes the
   right byte to the wrong slot; a missing `coarse` on a DMX profile is refused,
   but a *present wrong* one is not. Addresses are 0-based offsets from a
   1-based `address` — off by one is the default mistake.
3. **`range` on `cct` is kelvin, on everything else it is the float's own units.**
   `light_state` clamps into a `cct` window; `Channel::raw` remaps a byte out of
   the others.
4. **Order of channels in the file is irrelevant** — they are sorted by slot in
   `frame`. Order *within a channel* is coarse then fine, as a desk packs them.
5. **`data/schema.json` does not know about fixtures.** If a float is missing
   there it was missing from the *schema*, not from your profile.
6. **Do not widen a shared block to make one fixture work.** That is the exact
   mistake this document exists to prevent; it widens every model's row, moves
   every saved preset, and costs a golden-frame re-freeze. Use `a`/`b`, the
   profile's `[limits]`, or a node config first.
7. **Bluetooth and DMX halves are one model in the room.** `moving-head-beam`
   and the Godox profiles show the pattern: one profile = one wire. Do not put a
   `[bluetooth]` table on a DMX profile or `coarse` on a Bluetooth one — `parse`
   refuses both.

## 8. One-paragraph checklist

Copy the closest existing profile; change `label`, `[[channel]]`s and `range`s;
keep `wire`/`coarse` consistent with each other; run the `metal-perform` and
`metal-schema` tests; add the `[[fixture]]` row to the room with its own
`address`/`universe`/`group`; leave `data/schema.json`, `allowseed` and
`SIGNAL_FLOATS` alone. Only if a light needs a float the room's vocabulary
lacks: append to `SIGNAL_FLOATS`, add a `SIGNAL_WIDTH_BEFORE_*` anchor, register
it in both migration gap lists, walk §6.4, re-freeze the golden frames, and say
so in the commit.
