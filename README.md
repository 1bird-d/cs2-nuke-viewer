# cs2-nuke-viewer

A viewer for the instrumentation, piping and electronics of `de_nuke` in CS2.

**[Open it in your browser →](https://1bird-d.github.io/cs2-nuke-viewer/)** —
needs WebGPU. Nothing to install.

Counter-Strike 2's `de_nuke`, treated as the nuclear power station it is dressed
as. Everything that is not plant is taken away, and what is left — the pipework,
ducting, vessels, machinery, instrumentation and switchyard — stands on its own
in three dimensions, in the positions it occupies in the game.

Hover anything and it tells you what the geometry is, what real plant item it
most likely represents, and how good that match actually is. Where a real
counterpart exists, there is a link to a photograph of one.

> Counter-Strike and `de_nuke` belong to **Valve Corporation**. This project is
> unaffiliated, makes no money, and exists for information and education. See
> [NOTICE.md](NOTICE.md).

## Equipment names

Every identification comes from [`docs/ABWR-review.md`](docs/ABWR-review.md), a
survey of the map's props against what an ABWR actually contains. Each is tagged
with how strong the claim is:

| | |
|---|---|
| **IDENTIFIED** (green) | the prop genuinely depicts this — RPV closure head, spent fuel racks, dry casks, condensate storage tank, SF6 breakers, CTs, GSU transformers |
| **PROXY** (amber) | it does not depict it, but it is the best stand-in the map offers, and the mismatch is stated — the horizontal drums as MSRs or feedwater heaters |
| **MISMATCH** (red) | real equipment that contradicts the ABWR reading — the ovoid containment shells, the analogue control room, the domestic gas meter, the "Primary/Secondary Coolant" placard |

Props the review could not identify get **no label**, rather than a guess.

The reference links go to Wikimedia Commons and every one was resolved against
the Commons API before being written down; a test enforces the shape so a
half-remembered URL cannot creep in. Where the nearest verifiable photograph is
of the wrong reactor type, the link says so — the only reactor-vessel-head photo
on Commons is of PWR heads, and it is labelled as such.

## What the map turns out to be

The short version: **a generic nuclear station with a strong back end and no
front end.**

Genuinely there and genuinely accurate: the switchyard — SF6 dead-tank breakers,
current transformers, GSU transformers, lattice gantries, busbars, marshalling
kiosks. The spent fuel side: racks, pool, dry casks on a proper ISFSI pad with
marked spare positions. A condensate storage tank at the right order of
magnitude.

Absent entirely: the turbine island. There is no asset anywhere in de_nuke named
turbine, generator, condenser, pump, valve, feedwater, recirculation or cooling —
verified by parsing all 265 materials and 706 model names in the bake, not by
eyeballing. No reactor internal pumps, no FMCRDs, no standby gas treatment, no
radiation monitors.

Contradicting an ABWR outright: the signature ovoid shell. An ABWR containment is
a concrete cylinder with a **flat top slab**, entirely inside a rectangular
reactor building — you never see a dome. And the control room is 1970s analogue,
two design generations before the ABWR's digital one.

There is no deaerator, and there should not be: a BWR has none. The main
condenser deaerates, and the feedwater *is* reactor coolant.

## Running it locally

The desktop build reads the map out of **your own** Counter-Strike 2 install and
ships no game data at all. Windows:

```bash
nukeplant.bat
```

That builds, bakes the scene out of your CS2 files if it is missing or stale, and
opens the viewer. First run compiles wgpu and takes about a minute.

| | |
|---|---|
| `nukeplant.bat` | build, bake if needed, open the viewer |
| `nukeplant.bat rebake` | force a rebuild of the scene, then open |
| `nukeplant.bat bake` | force a rebuild and stop — no window |
| `nukeplant.bat status` | say whether the scene is current |

By hand:

```bash
cargo run --release -p bake --bin nkp-bake
```

```bash
cargo run --release -p view --bin nukeplant
```

### Controls

| | |
|---|---|
| `WASD` `Q`/`E` | fly; hold **right mouse** to look |
| `Shift` / `Ctrl` | sprint / creep; scroll changes base speed |
| **hover** | name what is under the cursor |
| **left click** | hold it in the panel, with links to photographs of the real unit |
| `1`–`9`, `0` | cycle a category: solid → ghost → hidden |
| `P` / `I` / `M` | process plant / pipes only / whole map |
| `G` / `X` | ghost everything hidden / hide everything ghosted |
| `[` `]` | ghost brightness |
| `L` | toggle the pinned equipment names |
| `F` | frame whatever is visible |
| `H` | hide the panel (names stay up) |
| `F12` | screenshot to `captures/` (desktop only) |
| `Esc` | quit |

### Categories and ghosting

Every instance is classified once at load from its Valve material path. Each
category is independently **solid**, **ghosted** or **hidden**, and independently
coloured.

Ghosted geometry draws as a fresnel shell — bright at grazing angles, invisible
face-on — depth-tested against the solid pass. That is what lets the building
stay as legible context instead of either hiding the plant or vanishing. It is
additive and therefore commutative, so it needs no sorting and cannot flicker
during a flythrough.

## First-time Pages setup

The deploy workflow builds and publishes on every push to `main`, but **Pages
has to be switched on once by hand** — a workflow token cannot enable it unless
Actions already has write permission, so the very first deploy has a chicken and
egg problem. Either of these fixes it:

- **Settings → Pages → Build and deployment → Source: GitHub Actions**, or
- **Settings → Actions → General → Workflow permissions: Read and write**

Then re-run the workflow. Until that is done the build succeeds and only the
publish step fails, with those instructions in the log.

## Working on the web viewer

```bash
cargo build --release --target wasm32-unknown-unknown -p view --lib
```

```bash
wasm-bindgen --target web --no-typescript --out-dir web/pkg target/wasm32-unknown-unknown/release/plant.wasm
```

```bash
node tools/serve.mjs
```

Then open <http://localhost:8080>. A plain file:// URL will not work — ES modules
and `fetch` both need an origin, and the wasm needs an `application/wasm`
content type.

To regenerate the scene the web viewer serves, on a machine with CS2 installed:

```bash
cargo run --release -p view --bin nukeplant -- --keep pipe,duct,vessel,machinery,instrument,electrical --export-web web/de_nuke.plant.nkp
```

```bash
gzip -9 web/de_nuke.plant.nkp
```

That is 8,625 instances down to 2,624, and 125 MB down to 37 MB, 9.4 MB gzipped.
CI does not rebuild it — a GitHub runner has no copy of the game — so it is
committed, and the workflow fails loudly if it is missing.

### WebGPU only

Per-instance transforms and colours come from a read-only storage buffer indexed
by `@builtin(instance_index)`. WebGL2 has no storage buffers, so there is no
slower fallback to offer; the renderer cannot be expressed. The page checks
`navigator.gpu` up front and explains, rather than showing a dead canvas.

## Where the geometry comes from

Straight out of your own CS2 install, read-only, through mapview's decoders.

`mapview/exports/de_nuke.glb` is *not* the input and cannot be. 5,371 of its
5,446 nodes carry no transform, so all 1,482 copies of the pipe kit render
stacked on the world origin, and only two nodes are named — leaving nothing to
classify pipes by. The cause is in mapview's `source2::world`:
`m_fragmentTransforms` entries are a flat array of twelve doubles, not three
nested rows of four, so the row-wise reader gives up and returns the identity.
[`crates/nkp-bake/src/world.rs`](crates/nkp-bake/src/world.rs) reads the raw KV3
and accepts both encodings.

Verified against an independent Source 2 Viewer decompile of the same map: both
put the 1,482 pipe fragments at 1,456 distinct origins.

## What is in the map

| | |
|---|---|
| Placements | 8,603 (364 scene objects, 4,748 aggregate fragments, 3,491 merged draw calls) |
| Instances | 8,625 |
| Unique geometry | 3,098,932 vertices / 2,600,414 triangles |
| Materials | 265 |
| Extent | 321 × 74 × 210 metres |

## Layout

```
crates/nkp-format   the .nkp container: layout, memory-mapped and in-memory readers
crates/nkp-bake     VPK resolver, world decode, geometry assembly
crates/bake         the bake CLI
crates/view         the viewer — shared library, desktop binary, wasm entry
crates/vendor       vpk and source2, copied from mapview (see its README)
web                 the page, and the scene it serves
tools/serve.mjs     a static server for local development
```

Vertices are stored in **Source units**, model-local; the conversion to Y-up
metres rides on each instance's matrix. That is what lets 1,482 pipe placements
share one copy of the pipe. Instance AABBs are already in world metres.

The coordinate convention matches mapview's: Source `(x, y, z)` becomes
`(x, z, -y)` scaled by one inch, so a position printed by either project means
the same place.

## Not done

Pipe connectivity — de_nuke's pipework is decorative modular kit placed to *look*
joined, so any run graph would be geometric adjacency, not an extracted P&ID.
Annotation proxies for the missing turbine island and the containment silhouette.
Textures. Camera-path capture for video.

## Licence

[MIT](LICENSE) for the code. Not for the game data — see [NOTICE.md](NOTICE.md).
