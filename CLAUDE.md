# Working on nukeplant

Rules that hold for every agent and every session, in the order that breaking
them causes trouble.

## Launching the viewer during development

**Open the window behind whatever the user is doing.** Any scripted launch —
screenshots, frame capture, a quick look at a change — must not take focus or
raise itself over the user's desktop. Pass `--unfocused`. A launch he starts
himself behaves normally.

**Never screen-capture to check a render.** Grabbing the screen region where the
window sits captures whatever is actually in front of it, which is the user's
desktop, not this program. This has already happened once.

Two supported ways to look at a frame:

* `--screenshot out.png` renders offscreen with no window at all. Use it for
  scene work.
* `--selftest out.png` opens the real window, lets it settle, captures the
  presented frame — panel included — and quits. Use it for anything involving
  the UI, since the offscreen path draws no overlay.

## Everything upstream is read-only

`a sibling mapview checkout` and
`C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\`
are **read only**. nukeplant depends on mapview's `vpk` and `source2` crates by
path and reads the user's own game files. It writes nothing to either, ever.

Only `de_nuke` is in scope. The rest of the map pool is mapview's business.

## Why we do not use `source2::world`

mapview's typed world decoder reads a transform as three nested rows of four.
That is how `m_sceneObjects` stores `m_vTransform`, but `m_fragmentTransforms`
on an aggregate is a **flat typed array of twelve doubles**. The row-wise reader
finds a `Double` where it wants an array and returns the identity, which
collapses 4,748 de_nuke placements — every one of the 1,482 pipes — onto the
world origin.

`crates/nkp-bake/src/world.rs` decodes the raw KV3 itself and accepts both
encodings. Everything else — VPK, KV3, VBIB/MBUF, meshopt, BCn — is mapview's
and is used unchanged.

The bake prints the placement spread of the largest aggregates on every run.
`n0_lr0_agg_prop_metal_pipe_001_0` must show **1,482 placements with 1,456
distinct origins**. One distinct origin means the regression is back.

## Builds

Use a private target directory so the user's warm cache is never invalidated:

```
$env:CARGO_TARGET_DIR = "$env:TEMP\nukeplant-agent-target"
```

## Determinism

Frame capture runs on a fixed timestep, not the wall clock: frame *i* is
`t = i / fps`. Rendering the same shot twice must produce byte-identical PNGs.
Anything added — animation, UI, loading — has to leave that true.

## Branching

Every stage is built on `stage/vX.Y.Z` and **merged by the user, never by an
agent**. Do not commit to `master`.

## Voice

Comments explain *why*, in plain declarative English, usually with the
measurement that motivated the choice. Match the file you are editing.
