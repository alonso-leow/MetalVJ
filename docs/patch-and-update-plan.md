# Plan — one clone, local patch, upstream updates

Goal: **one working clone** (this one, `hotel-fiesta`), the Fiesta Resort patch
kept as our own data, and `MetalVJ` updates pulled from the public repo without
ever touching the patch. Plus two fixes the schema work exposed: the last two
real "leaks" go upstream, and the pan/tilt `range = [-0.5, 0.5]` bug stops
being a bug.

Status of the research this rests on is in **§0**; do not start until §0 reads
true, because one of its facts (squashed history) decides the whole shape.

---

## §0. What is actually true right now (measured, 2026-10-26)

| Thing | Fact |
| --- | --- |
| Local repo | `C:\Users\DJ\visual-design-host\hotel-fiesta` |
| Local branches | `hotel-fiesta` (`48ec3a5`) and `main` (`bbbd223`) — **≤1 commit apart, same tree otherwise** |
| Local remotes | `origin` → `hotel-fiesta.git` (private), `experimental` → sibling dir |
| Public repo | `git@github.com:alonso-leow/metalvj.git`, remote name `upstream` (in the sibling) |
| Public branches | `upstream/main` = `7bf4b7e`, `upstream/dev` = `3944557` (`dev` = `main` + 1 English-translation commit) |
| **Public root commit** | `9362a86 "MetalVJ"` — **no parents** (squashed re-import) |
| **Local root commit** | `9ffeb60` — different |
| Shared ancestry | **none.** 433 local commits vs 3 public. Merge/ff is impossible. |
| Public tree vs local tree | identical **except** the hotel content, which the public `.gitignore` lists |
| Public `.gitignore` already ignores | `data/scenes/hotel-fiesta/`, `data/agent/venues/fiesta-resort.toml`, 13 hotel `data/fixtures/*.toml`, `web/wasm/data/graphs/hotel-fiesta.json`, `crates/metal-*/tests/{hotel_fiesta_graph,fiesta_patch}.rs` |
| Local hotel content size | 109 scenes (3.1 MB), 20 profiles (16 KB), 1 venue (32 KB), 1 graph (3 KB) |
| The leak, verified | Upstream `data/fixtures/moving-head-beam.toml` still says `range = [-0.5, 0.5]` on pan/tilt, and still **no `flex` float** exists despite a 40-channel head |

**The single most dangerous fact:** those hotel files are *tracked* locally but
*ignored* upstream. Any operation that takes upstream's tree wholesale — merge,
`reset --hard`, `checkout -f`, `clean -xdf`, a fresh clone — **deletes all 109
scenes and the room**. This has almost certainly already happened once (the
sibling `MetalVJ-Experimental` is at `bbbd223`, which *deleted* them; they exist
in `hotel-fiesta` only because this checkout is at `48ec3a5`).

So the plan's first rule: **the Fiesta patch never lives in a path upstream
ignores.**

---

## §1. Target layout (one clone)

```
hotel-fiesta/                       one clone, worktree of the public repo
├── (public tree, tracks upstream/dev exactly)
├── data/                           upstream's shipped data
│   └── fixtures/                   example-*, godox-*, moving-head-beam
└── patch/                          ← our content. NOT ignored by upstream.
    ├── fixtures/*.toml             13 hotel profiles + the 7 the public tree lacks
    ├── venues/fiesta-resort.toml
    ├── scenes/                     109 scenes, as today
    ├── graphs/hotel-fiesta.json
    └── tests/                      the two hotel tests, moved here
```

`patch/` is a **tracked** directory on a **local-only branch**, and no file under
it is named in upstream's `.gitignore`. That is the entire trick: ignore rules
are matched by path, and a path that upstream does not know about can never be
swept away by a tree update.

`data/fixtures/` keeps only the public examples. At startup the loader reads
`data/fixtures/` **and** `patch/fixtures/` — see §2.

### Why not the alternatives

- **`--assume-unchanged` / `skip-worktree`**: a lie to git. Survives nothing
  (`checkout -f`, `reset`, any clone), and a `git status` that cannot show a
  changed file is how a show gets rebuilt on the wrong profile. Rejected.
- **A second worktree of the same repo**: keeps history but gives you two
  checkouts and two `target/` dirs, which is the thing we were asked to remove.
- **Fork on GitHub + merge**: the histories are unrelated (§0), so every
  "merge" is a `--allow-unrelated-histories` conflict on 400 files, forever.
  It buys nothing a local branch does not.
- **Submodule for the patch**: adds a second clone. Rejected for the same reason.

---

## §2. The one code change upstream that makes the patch a first-class thing

Right now the patch only works because it sits in the same directory the engine
scans. Move it to `patch/` and `fixture::load` stops seeing it, so **upstream
needs a small, general feature** — not a hotel-specific hack:

> *More than one profile directory can be loaded, later directories overriding
> earlier ones by model.*

That is genuinely useful to everyone (a user's own profiles should not sit in
the repo's shipped `data/fixtures/` either — it is the same "operator's own
data" rule as `collections.json`), and it is what lets us keep the patch out of
an ignored path. It lands in three places:

1. `crates/metal-perform/src/fixture.rs`
   - add `pub fn load_with(root: &Path, extra: &[PathBuf]) -> Vec<Fixture>`
     that loads `dir(root)` first, then each extra, the last write per `model`
     winning; `load` stays a one-line wrapper over it (`extra = &[]`), so no
     caller changes.
   - the override is by model id, and a profile in an extra dir that fails to
     parse is still a skip-with-stderr (the existing rule, `fixture.rs:1`).
2. `crates/metal-dash/src/call/venue.rs` (or wherever `data_root()` is resolved)
   - resolve the extra list once: `patch/fixtures` when it exists, plus any
     `--fixtures <dir>` argument. Expose it in `GET /fixtures` so the Settings
     page can label a profile "local" vs "shipped".
3. the two `metal-perform` tests that walk `data/fixtures/` from disk
   (`fixture.rs:1394`, and `wire.rs`'s parse test) — they must use the same
   resolved list, or they will not cover the hotel profiles any more.

**This is the commit that makes the whole plan work.** Without it the patch has
to live in `data/fixtures/` and we are back to §0's deletion hazard.

---

## §3. Commits, and where they go

Three separate branches on the public repo's remotes. Do not mix them.

### 3.1 `upstream/dev` — the fixes everyone should have

Branch from `upstream/dev`, push to `upstream/dev` directly (it is the active
line; `main` is a release stamp). One PR-sized branch each, or one branch with
three commits — three commits is better, they are unrelated.

**Commit 1 — `fixture: load profiles from more than one directory`**
= §2, plus tests. No hotel content in it. This is a general fix.
Must also update `skills/add-fixture.md`'s "the file stem is the model id"
section to say where profiles are found.

**Commit 2 — `schema: document every append anchor in one place`**
The leak in its *cheap* half. Today an append means touching
`SIGNAL_WIDTH_BEFORE_*`, `migrate_signal_row`'s `from` array, and
`migrate_effects_row`'s `signal_gaps` array — three lists that must be kept in
step by hand, and the second is the one that gets forgotten (`lib.rs:2129` vs
`:2162`). Replace the two literal arrays with one:

```rust
/// Every width the signal block has ever had, oldest first. An append adds
/// exactly one entry here and nothing else: `migrate_signal_row` and
/// `migrate_effects_row` both derive their gap lists from it, so they cannot
/// disagree about how many old shapes exist.
pub const SIGNAL_WIDTHS: &[usize] = &[8, 13, SIGNAL_WIDTH];
```

and a test that asserts `SIGNAL_WIDTHS` is strictly increasing, ends at
`SIGNAL_WIDTH`, and that every prefix is reachable by `migrate_row` from a row
of that width. That test is the thing that makes the next append safe; today
nothing fails if you add one array entry and forget the other.

**Commit 3 — `fixture: pan/tilt channels map the whole signal travel`**
= §4, the `range` bug, plus the doc fix in `skills/add-fixture.md` §5 that
currently *tells people to reproduce it*.

### 3.2 `hotel-fiesta-patch` — our content, never upstream

A local branch, pushed to `origin` (the private repo) only. It holds `patch/`,
the moved tests, and nothing from upstream's files. If someone ever pushes it to
`upstream` it is a normal review failure — but it cannot leak hotel data into
the public tree by accident, because everything in it is new paths.

### 3.3 `main` — what ships

`main` fast-forwards `upstream/dev`. Never contains `patch/`. That is the release
line and the only branch a collaborator needs.

---

## §4. The pan/tilt bug — what it is, what "fixed" means

**The bug, stated exactly.** `Channel::raw` remaps the signal's own range onto
the channel's (or the float's) range. A pan channel with `range = [-0.5, 0.5]`
therefore turns `sig_pan ∈ [-1, 1]` into the **middle half** of the byte range,
and everything outside `[-0.5, 0.5]` clamps to the ends. Two consequences:

- with `PatchMode::Patch`, a row's `sig_pan` of ±1 lands at ±mid-travel, never
  at the head's real end stop; the head can only reach half its arc.
- the profile's `[limits] pan = [-270, 270]` is then unused for packing —
  the two ranges disagree and nothing warns.

`moving-head-beam.toml` even documents the opposite of what it does: *"`range`
is omitted everywhere because every channel uses its `sig_*` float's own
declared range"* — and then declares it on pan and tilt.

**Why it was written that way.** It was authored against a simulation whose
`Pose` was in metres/degrees around a nominal centre, and `[-0.5, 0.5]` made a
0.0 signal land at 0x8000 = "straight ahead". That is a *pose* convention, not a
*DMX* one, and it leaked into the profile.

**The fix** (three parts, all upstream):

1. **Data**: delete `range = [-0.5, 0.5]` from every pan/tilt channel in
   `data/fixtures/*.toml` (8 profiles locally: `beam-z-2`, `beamz2`,
   `cabeza-7r-230`, `example-moving-head`, `moving-head-beam`, `r7`, `wash-3`,
   `wash-escenario`). The float's default `[-1, 1]` is the correct mapping.
2. **Guard, so it cannot come back**: a `parse` warning (or a `metal-schema`
   lint, whichever is cheaper — `parse` already returns `Err` for ambiguity, and
   this is not ambiguous, it is merely almost always wrong). Cheapest honest
   version: `Fixture::parse` records it and `GET /fixtures` reports
   `"pan range narrower than the signal: the head can only reach half its
   travel"`. A hard error would break the stored hotel data before it is fixed,
   so: **warn, do not refuse.**
3. **Test**: `a_pan_channel_at_full_signal_reaches_the_end_stop` — build a
   profile with no `range`, assert `raw(sig.pan = -1.0) == 0` and
   `raw(+1.0) == 65535`; and with `[-0.5, 0.5]` assert it is *half*, documenting
   the old behaviour so the change is deliberate.

**Migration note**: the profiles are data, so nothing saved changes shape.
But a show authored against the centre-half mapping will now swing wider. Since
`fiesta-resort.toml` is all `drive = "fixed"` (§5), **no stored show moves** —
which is exactly why this is safe to fix now rather than later.

---

## §5. `drive = "fixed"` — deliberate, and the header should say so

The report says the 62 rows being `fixed` means "the signal-sourced per-fixture
path is unused". That is true and it is **not** the same kind of problem as §4:

- `drive = "fixed"` means the fixture takes its colour from the composited frame
  where the beam lands (`frame` sampling), which is the intended behaviour for
  a room that is lit *by* the projection. It is not broken.
- `drive` **is** a real venue-row field: `crates/metal-perform/src/venue.rs:1176`
  (`pub drive: Drive`, `Drive::Frame | Drive::Fixed`, default `Frame`). So all
  62 rows are *deliberately* frame-driven, and the `venue.fixture` `signal`
  path is simply not used by this room. That is a lighting decision, not a
  defect — the report's "unused path" is true but benign.

Action, on the patch branch (it is our data, not upstream's): record the reason
in the venue header (`fiesta-resort.toml`'s own preamble), so the next reader
does not re-open this. Optionally add `venue.fixture` nodes with
`drive = "patch"` for the handful of heads that should follow a row; that is a
show change, and it does not belong in the public tree.

---

## §6. The update loop (the whole point)

Do this in the **existing** clone. No second checkout, ever.

```powershell
# once: give the clone the public remote
git remote add upstream git@github.com:alonso-leow/metalvj.git
git fetch upstream

# the update itself — never a merge, the histories are unrelated (§0)
git checkout dev-local            # local branch tracking the patch stack
git rebase upstream/dev           # replay our few commits on the new public tip
```

Rebase works here because our commits are **few and small** (§2, §3.2, and the
patch branch is a tree of *new* paths). The rules that make it stay small:

1. **Our commits never edit a file upstream owns.** Everything we author lands
   under `patch/` or in the new `--fixtures` plumbing. The moment we edit
   `fixture.rs` we have created a conflict for every future update — so §2 and
   §4's guard go upstream *first*, and we consume them back.
2. **`data/fixtures/`, `data/agent/venues/`, `data/scenes/` stay pristine.** A
   profile we add goes to `patch/fixtures/`. This is what makes `git rebase`
   a fast-forward with nothing to replay.
3. **The public `.gitignore` keeps its hotel lines.** They are now belt-and-
   braces (nothing hotel lives in `data/` any more), but leaving them means a
   stray `cp` into the wrong path is still invisible to `git add -A`. Do not
   remove them.
4. **Never merge, never `reset --hard` upstream, never `clean -xdf`** without
   first checking `git status --ignored` for hotel paths. Add a script
   (`scripts/patch-status`) that prints `git status --ignored=matching` filtered
   to `patch/` and `data/`, so a "clean" checkout that ate the room is loud.

### The safety net, once

Before the first rebase, verify the patch is recoverable from outside the
working tree:

```
git bundle create ..\hotel-fiesta-patch.bundle hotel-fiesta-patch
```

and keep it wherever the private `origin` is already backed up. With `patch/`
tracked on a pushed branch, a lost checkout costs one clone — not a night.

---

## §7. Order of operations

1. **Nothing to settle.** `drive` is real (`venue.rs:1176`); §5 is a note to
   write into the venue header, not a blocker.
2. **Commit 1** (`load_with`) on a branch off `upstream/dev`; push to
   `upstream/dev`. Also update `skills/add-fixture.md` §2 to say profiles may
   come from a second directory.
3. **Commit 2** (`SIGNAL_WIDTHS`) same branch or a second; push. Now the leak
   table in `skills/add-fixture.md` §6.4 can shrink to "add one entry to
   `SIGNAL_WIDTHS`" and the doc stops describing a three-list dance.
4. **Commit 3** (pan/tilt `range`) — delete the 8 ranges, add the warn + the
   test; push. Fix `skills/add-fixture.md` §5, which currently *documents* the
   bug as correct.
5. **Rebuild the worktree**: convert this clone to track `upstream/dev`, move
   the hotel content into `patch/`, move the two hotel tests with it, and drop
   `git remote experimental` and the sibling directory.
6. **Patch branch**: commit `patch/`, push to private `origin`, make the bundle.
7. **Verify**: delete nothing yet — run `git status --ignored`, confirm 109
   scenes + 20 profiles + the venue + the graph are all *tracked* under `patch/`.
8. **Only then** remove `MetalVJ-Experimental` and `origin`(hotel-fiesta.git) if
   you want one remote; keep the bundle regardless.

Step 5 is the only destructive one. It must not run until steps 1–4 are pushed,
because the moment the worktree moves, the ignored-path hazard of §0 is live.

---

## §8. What this does *not* fix

- **The `flex` float does not exist.** A 40-channel head needs a float the room
  does not have. §6 of `skills/add-fixture.md` is the right place for that
  decision, and it is *not* in this plan — it is a show decision, and the point
  of §3.1 commit 2 is that when it is made, it costs one line and one test.
- **`config.toml` / `[studio]` reindex** is untouched. If the patch grows a
  timeline or a palette, `scripts/reindex.py` still has to be run by a person;
  that is deliberate (it is a reviewable edit), and nothing here changes it.
- **The `venue.fixture` `signal` path stays unused** in this room, because every
  row is `drive = "fixed"` on purpose. A lighting choice, not a defect; §5 says
  where to record it.
