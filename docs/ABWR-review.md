# de_nuke as an ABWR — equipment and instrumentation review

**Subject:** Counter-Strike 2 map `de_nuke`, baked as `scenes/de_nuke.nkp` (8,625 instances, 265 materials,
2.60 M unique triangles, extent 321 × 74 × 210 m).
**Question:** what ABWR plant equipment and instrumentation is actually present, and what is missing?
**Coordinate convention:** world metres, **Y-up**, matching the viewer. Scene bounds
`[-155.3, -30.9, -63.4] .. [165.8, 42.9, 147.0]`. Outdoor grade sits around **Y ≈ −7**; the fuel-pool
water surface is at **Y = −20.6**; the tallest thing in the map is the large containment shell apex at
**Y = +42.9**.

Evidence images are in `docs/evidence/`. Every claim below names the asset that proves it.

---

## 1. Executive summary

**Is this recognisably an ABWR? No — and the gap is structural, not cosmetic.**

What the map *does* have is a genuinely credible mid-20th-century **thermal power station with a nuclear
dressing**: a spherical containment shell, a submerged reactor-vessel closure head and a spent-fuel rack
array in a real water pool, dry-cask-like vertical silos on a hardstand, an analogue benchboard control
room with annunciators and strip-chart recorders, a full HV switchyard with breakers, CTs and three
oil-filled transformers, ~1,900 pipe placements, ~360 duct placements, 72 conduit/junction boxes and
radiological signage that names contamination and radioactive waste. As a *nuclear plant* set it is
better stocked than you would expect.

As an **ABWR** it fails on four counts, in descending order of severity:

1. **The containment is wrong, and it is the map's most recognisable feature.** The two green shells
   (`nuke_silo_001b`, 35 m across × 41 m tall at X≈17, Z≈−6; `nuke_silo_001`, ~22 m across × ~31 m tall
   at X≈60, Z≈24) are **spherical/ovoid free-standing shells on cylindrical skirts, with four external
   risers running up the outside and over the crown into an apex plinth** — see
   [`evidence/domeall.png`](evidence/domeall.png), [`evidence/domecrown.png`](evidence/domecrown.png),
   [`evidence/elev_north.png`](evidence/elev_north.png). The closest real-world analogue is a **steel
   spherical containment** (Gundremmingen A, Lingen, Kahl) or, frankly, a **spherical gas holder**.
   An ABWR containment is an **RCCV** — a reinforced-concrete cylinder ~29 m inside diameter with a
   steel liner and a **flat top slab** that forms the refuelling floor, wholly enclosed inside a
   rectangular reactor building. **From outside an ABWR you see a plain concrete box. You never see a
   dome.** Kashiwazaki-Kariwa 6/7 look like warehouses. There is no way to shoot the map's silhouette
   and call it an ABWR; the silhouette has to be explained away in narration or reframed as "a
   containment shell", full stop.
   The presence of **two shells of unequal size** compounds it: it reads as a main unit plus a second,
   half-scale one, which is not a configuration any BWR site has.

2. **Nothing in the map is BWR-specific, and the one detail that speaks says "PWR".** There is no
   suppression pool, no wetwell, no horizontal vent, no diaphragm floor, no steam tunnel, no drywell
   head, no SRV tailpipe or quencher. Conversely, the circuit-directory placard baked into the
   `nuke_industrial_props_001` texture (visible in
   [`evidence/lowerx_y25.png`](evidence/lowerx_y25.png) area panels) literally lists
   **"Primary Coolant"** and **"Secondary Coolant"** as separate circuits — a two-loop, PWR/steam-plant
   vocabulary. A BWR has one coolant circuit; feedwater *is* reactor coolant. An informed viewer who
   freeze-frames that panel will catch it.

3. **The balance of plant is essentially absent.** There is no turbine, no generator, no condenser,
   no moisture separator reheater, no feedwater heater string, no condensate pump, no condensate
   polisher, no reactor feed pump. I verified this by exhaustive name search across all 265 material
   paths, all 8,625 instance names and all 715 decompiled texture names: the strings
   `turbine`, `generator`, `condens`, `boiler`, `pump`, `valve`, `heater`, `cooling`, `deaerat`,
   `separator`, `dryer`, `diesel`, `feedwater`, `recirc` appear **zero times**. The only "steam" in the
   map is the `steam_001_additive` particle material (7 plume instances). The largest structure on a
   real ABWR site — a ~110 × 60 × 40 m turbine hall — has no counterpart here.

4. **The instrumentation that *is* modelled is the wrong generation.** The map's control room and its
   panel banks are **analogue benchboards**: round moving-coil indicators, vertical edgewise strip
   meters, three-position selector switches, illuminated pushbuttons, a 6 × 4 backlit annunciator tile
   matrix and a **strip-chart recorder with a visible trace** — all in the
   `control_room_displays_color` texture atlas. That is a 1970s BWR/3–5 control room. An ABWR MCR is
   fully digital: a wide fixed display panel, flat-panel touchscreen operator consoles, SSLC in four
   divisions, and an automated plant-startup capability. The map's kit is excellent *set dressing for
   instrumentation in general* and completely wrong *for an ABWR specifically*.

**On the deaerator question the user raised specifically: an ABWR does not have one, and its absence
from the map is not a defect.** See §6.1 — the answer is subtler than present/absent and is worth
saying on camera, because it is a genuinely counter-intuitive piece of BWR design.

**Best-case framing for the video.** The map supports a *credible* nuclear plant narration at the level
of: containment, fuel pool, fuel racks, vessel head, dry-cask store, switchyard, control room, service
water and HVAC. It does **not** support a claim of ABWR fidelity without annotation proxies for at
minimum: the RCCV, the RIPs, the FMCRDs, the four main steam lines and the entire turbine island.

---

## 2. Method

Everything below was verified against the baked scene or the read-only decompile — nothing is from
memory of the map.

1. **Full asset inventory.** Parsed `scenes/de_nuke.nkp` directly in Python against the documented
   binary layout (120-byte header, 104-byte instance records, 32-byte material records, UTF-8 string
   table). Dumped all **265 material paths** with per-material instance and triangle counts, and all
   **distinct model stems** with counts. This is the backbone of the review.
2. **Positional queries.** For each candidate asset, listed every instance with its world AABB centre
   and size, so "present" claims carry coordinates and dimensions.
3. **Direct visual inspection.** Rendered offscreen with
   `target/release/nukeplant.exe --screenshot` (never screen-captured; never focused a window).
   Roughly 105 renders retained under `docs/evidence/`.
   - Whole-map plan and four elevations.
   - Per-prop isolation renders from two to four orbit angles.
   - **Spatial cross-sections.** The viewer has no clip-box, so I wrote a small script that copies the
     `.nkp` and rewrites only the header instance count, scene bounds and the instance table, keeping
     instances whose AABB centre falls in a chosen world box and optionally dropping building fabric,
     clutter and lighting by material regex. That gave clean cutaway views of the lower reactor level,
     the control room, the switchyard and the pool. **No file in the nukeplant source tree was
     modified**; the crops lived in the scratchpad and have been deleted.
4. **Texture forensics.** Read the colour textures out of the read-only Source 2 Viewer decompile at
   `` (715 PNGs) and the per-prop decompiles
   (`machinery1|2|3`, `pipes`, `nukescreenshots`). Texture atlases are the single best evidence of what
   a low-poly prop is *meant* to be — the `control_room_displays`, `nuke_industrial_props_001`,
   `metal_pipe_002b`, `gas_meter_01` and `signs_001` atlases each settled a question that geometry
   alone could not.
5. **Negative search.** Grepped the 265 material paths, the instance-name table and the 715 texture
   filenames for every plant-equipment keyword in the checklist. Results in §3.0.

**Note on the toolchain.** The viewer binary was rebuilt by the user mid-session and its CLI changed
from `--only/--hide/--tint` to `--preset/--solo`. Renders made before the change (the `p_*`, `m0*`,
`plant_core_*`, `dome_*`, `poolx_*` files) use the old flat-shaded "everything" path; renders after it
use the category colouring — **orange = pipework, blue = HVAC duct, green = vessels/silos,
purple = "instrumentation" category, yellow = electrical, grey = building fabric**. That colour key is
the project's classification, not mine, and §4.6 notes where I think it mis-sorts.

---

## 3. System-by-system inventory

### 3.0 Keyword audit (reproducible negative evidence)

Searched across 265 material paths, 8,625 instance names, 715 decompiled texture filenames:

| Keyword | material paths | instance names | texture names |
|---|---|---|---|
| valve, turbine, boiler, pump, cooling, condens, generator, heater, deaerat, separator, dryer, diesel, recirc, feedwater, stack, chimney, detector, radiation, transmitter, gauge | 0 | 0 | 0 |
| steam | 1 (`effects/smoke/steam_001_additive`) | 7 | 1 |
| monitor | 1 (`nuke_office_desk_monitor`) | 2 | 3 |

So: **no asset in de_nuke is named for any item of rotating machinery, heat-exchange equipment, valve or
radiation instrument.** Everything identified below is identified by *shape and texture*, not by name.

---

### 3.1 Nuclear island

| Item | State | Evidence |
|---|---|---|
| **Reactor pressure vessel** | **Absent** as a vessel. **Present** as a closure head only. | `nuke_reactor_vessel_head_color` — **1 instance, 3,014 tris**, centre **(−3.6, −26.8, 22.8)**, 6.6 × 3.9 × 6.6 m. A circular studded closure flange with a raised bolted stub, **sitting on the floor of the fuel pool** (pool water at Y −20.6, pool floor ≈ Y −27.5). [`evidence/p_nuke_reactor_vessel_head.png`](evidence/p_nuke_reactor_vessel_head.png), [`evidence/rpvhead_y25.png`](evidence/rpvhead_y25.png), [`evidence/poolplan.png`](evidence/poolplan.png). There is **no vessel shell** anywhere: nothing cylindrical of ~7 m diameter and ~21 m height exists in the map. |
| Reading of that prop | — | It is a **head laid down under water during a refuelling outage** — which is a legitimate and rather good refuelling-floor scene. But 6.6 m diameter is close to an ABWR RPV flange (7.1 m ID), so the scale is actually not bad. Narrate it as the head, not the vessel. |
| RPV internals (shroud, core plate, top guide) | **Absent** | No internal geometry of any kind. The head prop is a solid closed mesh. |
| **Steam separators and dryers** | **Absent** | No asset. Nothing on the refuelling floor resembles a separator or dryer stand-off. |
| **Ten Reactor Internal Pumps (RIPs)** — the ABWR signature | **Absent** | Nothing under the vessel head; no RIP motor casings, no purge lines, no RIP motor cooling headers. There is also **no external recirculation loop, no jet-pump riser and no recirc piping**, which is technically *correct* for an ABWR — but it is correct by omission, not by depiction. A viewer cannot see the absence of something that was never drawn. |
| **FMCRDs (205 fine-motion control rod drives)** | **Absent** | Nothing under-vessel. Nearest stand-in: the **four vertical stub risers with flanged tops** visible at (≈8–13, −20, 40–50) in [`evidence/lowerx_y205.png`](evidence/lowerx_y205.png) — four short flanged standpipes in a row. A cluster of them at high density could be dressed as a CRD grid, but there are four, not 205, and they are 1.5 m apart. |
| CRD hydraulic control units | **Absent** | No HCU racks, no scram accumulators, no charging water header. |
| **Four main steam lines** | **Stand-in available (poor)** | The **four large-bore risers that climb the outside of the big containment shell and turn over the crown** — `metal_pipe_001` fragments at (25.6, 24.2, −5.8), (25.6, 26.5, −7.3), (18.2, 29.4, 2.4), reaching Y = 37.8 m. [`evidence/domecrown.png`](evidence/domecrown.png). Four lines of the right relative diameter, on the containment, is a tempting proxy — but they run **over the roof of the containment** into an apex plinth, which is the opposite of how MSLs are routed (horizontally out of the RPV upper shell, through the drywell wall, into the steam tunnel at operating-deck level). Usable only with an explicit "these stand in for" caveat. |
| **MSIVs (8: inboard + outboard per line)** | **Absent** | No valve geometry anywhere in the map. |
| **SRVs, tailpipes, quenchers** | **Absent** | No valve geometry; no suppression pool for them to discharge into. |
| **Main steam tunnel** | **Absent** | No shielded horizontal chase from the containment to any turbine building. |
| **Spent fuel pool** | **Present** | `nuke_water` — 5 surface quads at **Y = −20.6**, spanning **X −7.2 … +21.8, Z 18.0 … 33.2** (≈29 × 15 m), with `caustics_001_decal` overlays. Pool floor ≈ Y −27.5, so **≈6.9 m water depth**. A real BWR SFP is ~11–12 m deep; this is shallow but reads correctly on camera. [`evidence/poolplan.png`](evidence/poolplan.png), [`evidence/lowerx_y25.png`](evidence/lowerx_y25.png). |
| **Spent fuel racks** | **Present** | `nuke_spent_fuel_racks_color` — **1 instance, 4,100 tris**, centre **(10.8, −27.5, 22.8)**, 17.1 × 0.4 × 8.5 m. A flat plate array of square cells in two sub-blocks on the pool floor. [`evidence/p_nuke_spent_fuel_racks.png`](evidence/p_nuke_spent_fuel_racks.png). Cell pitch scales to ~1.7 m — far too coarse for real BWR rack pitch (~15 cm) — but the read is unambiguous. |
| **Refuelling machine / fuel handling** | **Stand-in available (fair)** | Suspended from the ceiling above the pool at ≈(0–12, −22 … −25, 20–30) are several **long-handled tools, a mast with a head, and a hoist with a grab** (`nuke_industrial_props_001/002` fragments). [`evidence/rpvhead_y25.png`](evidence/rpvhead_y25.png). No bridge, no telescoping mast on rails, no refuelling machine gantry. |
| **Reactor building crane** | **Absent** indoors; **stand-in available** outdoors | `nuke_cargo_crane_base/cart/winch` — an **outdoor rail-mounted gantry**, ~39 m span, 14.6 m tall, two trolleys, hook block and festoon cabling, at **X −27.7, Z −40 … +3**. [`evidence/crane_y60.png`](evidence/crane_y60.png). Good heavy-lift/cask-handling crane; wrong location for an RB crane over the refuelling floor. |
| **Refuelling floor / operating deck** | **Partial** | The pool hall exists (floor ≈ Y −19.6, mimic-panel level Y −14.6, ceiling above), but there is no reactor well adjacent to the pool, no cavity gate, no dryer/separator storage pit. |
| **Equipment hatch, personnel airlock** | **Absent** | No containment penetration geometry. `nuke_industrial_silo_door_001` exists in the decompile node list but is a plain door. |
| **Dry cask storage** | **Present** — and a strong find | `medium_silo_color` — **8 instances, 15,408 tris**, on a marked concrete pad at **X 107 … 129, Z −20 … +23, Y −10.4 … −3.2**; each **5.3 m dia × 7.1 m tall**, a vertical cylinder with a bolted lid and flanged base, standing on a pad that has **recessed circular positions marked for more casks**. [`evidence/mediumsilo.png`](evidence/mediumsilo.png). This reads immediately as an **ISFSI pad with vertical overpacks**. Slightly fat versus a real HI-STORM (≈3.4 m dia × 5.9 m) but the composition — grid array, spare marked positions, open hardstand — is exactly right. Not ABWR-specific, but it is the single most convincingly "nuclear" thing in the map after the fuel racks. |

---

### 3.2 Containment and structures

| Item | State | Evidence |
|---|---|---|
| **ABWR RCCV** (concrete cylinder, steel liner, flat top slab, inside a rectangular RB) | **Absent** | See §1. Nothing in the map is a concrete cylindrical containment. |
| **What the map actually has** | — | **Large shell:** `nuke_silo_001b`, 5 merged fragments, **12,032 tris**, bbox `[−0.8, 1.7, −24.3] .. [34.4, 42.9, 11.3]` → **35.2 m across × 41.2 m tall**, ovoid on a cylindrical skirt with buttresses, ribbed meridians, a railed platform at the springline, a lightning finial and two down-conductors, an apex plinth with railing, and four external risers over the crown plus two more into the plinth on the east side. **Small shell:** `nuke_silo_001` (X > 45), 19 fragments, **13,760 tris**, bbox `[49.2, −8.9, 13.4] .. [71.3, 22.6, 35.2]` → **≈22 m across × ≈31 m tall**, same family, same crown risers. [`evidence/domeall.png`](evidence/domeall.png), [`evidence/east_y205.png`](evidence/east_y205.png), [`evidence/elev_north.png`](evidence/elev_north.png). Both are **single-sided hollow shells with no interior** ([`evidence/stack_y210.png`](evidence/stack_y210.png) shows straight through the back face). |
| **Suppression pool / wetwell** | **Absent** | Nothing toroidal, annular or pool-like at containment level. The only water bodies are the fuel pool (Y −20.6) and the river (Y −12.2). |
| **Horizontal vents, diaphragm floor, upper/lower drywell** | **Absent** | No internal containment geometry at all. |
| **Reactor building** | **Present in substance, wrong in form** | The central block spanning roughly **X −24 … +56, Z −25 … +62** over four levels — pool level (Y −28 … −20), lower plant floor (Y −19.6), hall/mimic level (Y −16 … −13), control/main floor (Y −10.5), roof (Y −6 … +7). Dense pipework and ducting throughout ([`evidence/lowerx_y25.png`](evidence/lowerx_y25.png), [`evidence/lowerx_y205.png`](evidence/lowerx_y205.png)). It is a plausible reactor-auxiliary building but it does **not** enclose the containment — the shells stand outside it. |
| **Turbine building** | **Absent** | No building of turbine-hall proportions, no crane rail at high level, no condenser pit, no TG pedestal. |
| **Control building** | **Present** | The MCR block at **X −9.2 … +6.8, Z −18.6 … +6.4, Y ≈ −10.5**, plus adjoining offices. §3.5. |
| **Radwaste building** | **Absent** | No tank farm inside a shielded building, no drumming station, no evaporator. Signage says "TOXIC STORAGE" and "DECONTAMINATION" (`hr_area_signs_decal_001` atlas) but nothing is dressed as radwaste plant. |
| **Plant vent stack** | **Absent** | No tall stack. Nearest thing: the four crown risers reaching Y +37.8, and a handful of **roof vent cowls with weather caps** on plinths at (≈10–25, +2 … +5, 5–20) — [`evidence/hall_y25.png`](evidence/hall_y25.png). An ABWR has a plainly visible ~100–150 m plant vent / RB stack; there is nothing of the sort. |
| **Steam plumes** | **Present** (cosmetic) | `steam_001_additive`, 7 instances, largest at **(14.8, ~26, 50.7)** rising to Y +39.6, and one at (−53.5, …, 73.4). Useful for a "plant is running" cutaway; they are unattached to any equipment. |
| **Ultimate heat sink** | **Present** | `hr_river_water_001` — 3 surfaces at **Y = −12.2**, spanning **X −155 … +139, Z 68 … 106** — a **294 m wide river across the whole site**, south of the plant. [`evidence/map_plan_all.png`](evidence/map_plan_all.png). Excellent once-through cooling-water source. |
| Access steel | **Present, abundant** | `web_joist_001` 186 + 51 instances (231 k tris), `catwalk_support_001` 6, `nuke_catwalk` 1, `metal_ladder_001/001b` 87, `metal_railing_001/001b` 112 + 54 cards, `metal_trim_001/002` 88, `nuke_floor_hatch` 9, `nuke_metal_hatch` 13. Category total: **499 instances / 415 k tris**. This is the map's strongest asset for making any annotation proxy look native — the platforms, ladders and handrails to hang new equipment off are already there. |

---

### 3.3 Safety systems

Every ABWR safety system is **absent**. This is the cleanest section of the review because there is
nothing to argue about: none of the following has any geometry, texture or asset name in the map.

| System | State | Nearest stand-in, if any |
|---|---|---|
| **RCIC** (steam-turbine-driven injection) | Absent | None. No turbine, no steam supply line, no CST suction. |
| **HPCF, two divisions (B and C)** | Absent | None. |
| **RHR, three divisions** (LPFL / SP cooling / shutdown cooling / containment spray) | Absent | The **horizontal drums** of `nuke_machinery_05` (§3.6) could be dressed as RHR heat exchangers — they are the right silhouette for a shell-and-tube exchanger, though there are only four and they are in the wrong rooms. |
| **SLC** (standby liquid control) | Absent | The **bolted vertical tank** `nuke_silo_002` (§6) could be captioned as an SLC storage tank, but at 11 m diameter it is two orders of magnitude too large; an SLC tank is ~15 m³. |
| **ADS** (8 of 18 SRVs) | Absent | No valves exist in the map. |
| **Three-division emergency diesel generators** | Absent | Nothing engine-like. The only rotating-machine prop is `nuke_machinery_03` (§3.6), one horizontal motor, in the wrong place and one-off. No exhaust stacks, no day tanks, no radiators, no EDG building. |
| **CST** (condensate storage tank) | **Stand-in available (good)** | `nuke_silo_002`, §6. |
| **Three-division physical separation** | Absent | No divisional colour coding on cable trays, no separated ECCS rooms, no divisional labelling. The one place divisional thinking is even hinted at is the circuit-directory placard in `nuke_industrial_props_001`, which lists "SUB D1…D4", "BKUP 1/2" and "REROUTE" — a redundancy/backup vocabulary, but not three-division ABWR separation. |
| **FCS** (hydrogen recombiners) | Absent | None. |

---

### 3.4 Auxiliary systems

| System | State | Evidence |
|---|---|---|
| **CUW** (reactor water cleanup) | Absent | No filter/demin vessels, no regenerative/non-regenerative heat exchanger pair. |
| **FPC** (fuel pool cooling and cleanup) | Absent | The pool has no visible skimmer weir, suction, return diffuser or piping connection. Pipework passes over the pool but does not terminate in it. |
| **SPCU** (suppression pool cleanup) | Absent | No suppression pool. |
| **RCW / RSW / RBCW cooling water chains** | **Stand-in available (good)** | The **three large-bore headers** (≈1.5 m dia, flanged, with expansion couplings) running east–west at **X ≈ −35 … −8, Z ≈ 30 … 45, Y ≈ 0**, turning 90° down onto pads on a low building roof, plus a **three-pipe manifold with in-line bulges at the building face**. [`evidence/cw_y290.png`](evidence/cw_y290.png), [`evidence/west_y20.png`](evidence/west_y20.png), [`evidence/solo_pipe_iso.png`](evidence/solo_pipe_iso.png). Three parallel large-bore lines terminating at a structure by a river is the single best "service water / circulating water" composition the map offers, and it is genuinely good on camera. |
| **HVAC** | **Present, abundant — but commercial-building HVAC** | `airduct_hvac_001` **295 instances / 133 k tris** + `airduct_hvac_001b` 4; `nuke_ventilation_exhaust_color` 15, `_small` 25, `_002` 19; `nuke_vent_slats` 1; `hr_metal_duct_001` 5; `nuke_roof_ac01/03/04/_box/_fan_low01/_base01/_base02/_inset` 44 combined. Category total **364 instances / 149 k tris**, extent 199 × 31 × 90 m. [`evidence/vents_close.png`](evidence/vents_close.png), [`evidence/solo_duct_iso.png`](evidence/solo_duct_iso.png). These are **rooftop air-handling units and rectangular sheet-metal duct** — office/industrial HVAC. |
| **SGTS** (standby gas treatment) | Absent | No filter train, no charcoal adsorber bank, no HEPA housing, no dampered bypass, no exhaust to a stack. The HVAC above cannot stand in for it — SGTS looks like a shielded filter housing train, not ductwork. |
| **Offgas system** (SJAE → recombiner → charcoal beds) | Absent | No vessels of the right form, no charcoal vault, no delay bed. |
| **Radwaste** | Absent | See §3.2. |
| **Instrument and service air** | **Stand-in available (weak)** | The horizontal drums of `nuke_machinery_05` read equally well as air receivers. No compressors, no dryers, no receivers explicitly. |
| **Fire protection** | **Present** | `sprinkler_001` 32 instances / 6,784 tris; `hotel_ceiling_firealarm001` 14; `nuke_office_firealarm_001_cover` 11; `nuke_fire_alert_light` 23; `nuke_fire_extinguisher` 27; `fire_hose_wa` 11. A complete building fire system — sprinkler heads, detectors, alarm beacons, extinguishers, hose reels. Not nuclear-specific, but real and dense. |
| **Drains** | Present (cosmetic) | `nuke_drain_covers` 15 instances. |

---

### 3.5 Instrumentation and control room

Treated in full in **§5**. Summary of the physical assets:

| Item | State | Count / location |
|---|---|---|
| Main control room, analogue benchboard + vertical panels | **Present** | `control_room_displays_color`, **46 instances** in the room bounded **X −9.2 … +6.8, Z −18.6 … +6.4, Y −9.8 … −7.5** (floor ≈ Y −10.5). [`evidence/ctrlroom_in3.png`](evidence/ctrlroom_in3.png), [`evidence/ctrlroom_plan.png`](evidence/ctrlroom_plan.png). |
| Reactor-hall mimic panel banks | **Present** | Same material, **5 instances** at Y −14.6, forming three wall-mounted banks: 14.2 m on the north wall (Z 34.1), 14.2 m on the south wall (Z 9.6), 11 m on the east wall (X 24.5). Bbox `[−10.0, −15.7, 9.6] .. [27.5, −13.5, 34.1]`. [`evidence/crd_hall_iso.png`](evidence/crd_hall_iso.png). |
| Local electrical / instrument panels | **Present** | `nuke_electric_panel01` 4 instances (floor-standing switchgear cubicle with ribbed door and side lamp stack), `nuke_electric_panel02` 14 (small wall panel with two indicator windows and a 4-lamp column). Spread X −64 … +68, Y −18.5 … −0.8. |
| Conduit and junction boxes | **Present** | `power_outlet_campground` — **72 instances / 30,240 tris**, bbox `[−58.6, −10.6, 0.4] .. [89.3, −3.7, 57.1]`. Texture is a **wall-mounted junction box with rigid conduit and cable clamps**, not a socket. This is the map's field-wiring signature. |
| Cable trays | **Texture only** | A scalloped ladder-tray strip appears in the `nuke_industrial_props_001` atlas but I could not confirm dedicated cable-tray geometry. Marked uncertain; resolving it means dumping the `nuke_industrial_props_001` sub-mesh list, which the aggregate bake does not preserve. |
| Local pressure/temperature gauges on pipework | **Present** | The `metal_pipe_002b` texture atlas contains a **bimetallic thermometer (0–40 °C)** and a **compound pressure/vacuum gauge marked in mbar and in.Hg**, plus bolted flanges, a blind flange, a bolted manway and a **flexible bellows/expansion joint**. Applied across the `metal_pipe_002` family (80 instances / 69 k tris). |
| CRT monitors / consoles | **Present** | `nuke_office_desk_monitor` 13 instances / 19,754 tris (boxy CRTs on desks); `nuke_office_desk_buttons` 6 instances (console button/keyboard array with coloured key blocks); `nuke_supercomputer_01/02` 2 instances (louvred equipment cabinets) at **(−6.4 … 5.7, −10.6 … −8.1, −16.3 … −12.2)**. |
| Domestic gas meter (**not** plant instrumentation) | Present, mis-categorised | `gas_meter_01/02/pipes` — 7 instances at **(25.2, −9.1, 11.8)**. The `gas_meter_01` texture is unmistakably a **diaphragm natural-gas meter with an m³ odometer register**. See §4.6. |
| Radiation monitors, ARMs, containment atmosphere, H₂/O₂, seismic | **Absent** | Nothing. See §5.4. |

---

### 3.6 Vessels and rotating machinery actually present

These four `nuke_machinery_*` props are the only process-equipment geometry in the map and they are much
better than the asset names suggest.

| Asset | Instances / tris | What it actually is | Where |
|---|---|---|---|
| `nuke_machinery_05` | 6 / 30,080 | **Horizontal cylindrical pressure vessel** — dished heads, **saddle supports**, a bolted flanged manway on the head end, top nozzle bosses, plus a slim companion cylinder (~0.5 m) alongside. Textbook process drum. [`evidence/m05_a_y40.png`](evidence/m05_a_y40.png), [`evidence/m05_c_y40.png`](evidence/m05_c_y40.png) | Two ~19 m × 3 m drums side by side at **(−11.4, −8.9, −3.6)** and **(−11.4, −8.9, −10.0)**; one 12.4 m drum at **(39.6, +3.3, −13.2)** on the containment plinth, piped into the shell; one 12.4 m drum at **(8.5, −17.8, 48.3)**. |
| `nuke_machinery_01` | 15 / 28,474 | **Vertical domed-top storage tank** with two lifting lugs. Used at two scales: **3.7 m dia × 5.4 m** (4 instances at (3.5, +4.5, −11.6) and (3.5, +4.5, −2.7), on a roof), and **24 m dia × 11 m** with a handrail ring at the dome springline (**5 instances** in the tank farm at **Z 116 … 147**, spread X −134 … +132). [`evidence/m01_tank_y30.png`](evidence/m01_tank_y30.png), [`evidence/m01_small_close.png`](evidence/m01_small_close.png) | Tank farm is south of the river, outside the plant fence. |
| `nuke_machinery_02` | 2 / 9,952 | **Rounded skid-mounted machine enclosure / acoustic hood** on an anchored base frame with lifting lugs and end handrails. ~5 × 3 × 10 m. [`evidence/m02_close_y30.png`](evidence/m02_close_y30.png) | **(15.7, −17.6, 57.3)** and **(36.0, −15.6, 37.8)**. |
| `nuke_machinery_03` | 1 / 8,552 | **Horizontal electric motor on a concrete plinth** — cylindrical stator, end bell with shaft stub and coupling, terminal box, cooling ribs. The only rotating machine in the map. [`evidence/m03_close_y210.png`](evidence/m03_close_y210.png) | **(7.1, −15.4, 23.0)** — one large (~3 m) and one small copy. |
| `nuke_industrial_props_001` | 87 / 61,474 | A kit: **distribution board with two columns of MCBs**, **relay/contactor cabinet with visible wiring and terminal blocks**, **6-lamp illuminated pushbutton sub-panel with a round gauge**, flush distribution panel, **cable-tray strip**, receptacle, plus stairs, grating and small platforms. The **circuit-directory placard** lives here. | Concentrated **X −32 … +72, Y −19.6 … −1.7, Z −24 … +60**. |
| `nuke_industrial_props_002` / `_002b` / `_003` | 29 + 2 + 2 | Structural: gratings, small platforms, standpipes, floor plate. `_003` is walkway grating. [`evidence/p_nuke_industrial_props_003.png`](evidence/p_nuke_industrial_props_003.png) | Widespread. |

---

### 3.7 Electrical

The strongest system in the map by a wide margin.

| Item | State | Evidence |
|---|---|---|
| **HV switchyard** | **Present, good** | Bbox `[22.5, −10.5, −18.1] .. [86.6, 4.6, 40.6]`, ≈64 × 15 × 59 m. Contains: **`nuke_circuit_breaker`** 8 instances / 33,722 tris — pedestal-mounted **dead-tank SF₆ breakers with V-arranged bushings**; **`current_transformer`** 17 / 46,662 — pedestal instrument-transformer columns; **`substation_support`** 29 / 20,190 — lattice gantries and bus support insulator columns; **`transformer_wires`** 36 / 32,322 — overhead bus and jumpers; **`transformer_yard_powerbox`** 15 / 7,874 — marshalling kiosks. [`evidence/switchyard_close_y30.png`](evidence/switchyard_close_y30.png), [`evidence/switchyard_y205.png`](evidence/switchyard_y205.png). |
| **Main / unit transformers** | **Present** | `substation_transformer_color` — **4 instances / 5,558 tris**, three at **X 45.8, Z −14.8 / −7.5 / +0.3, Y −8.1**, each **4.3 × 4.5 × 4.8 m**: oil-filled tanks with radiator banks and HV bushings, risers into the switchyard bus, against the building wall. [`evidence/maintx_y40.png`](evidence/maintx_y40.png). Three in a row reads equally well as GSU + two unit auxiliary transformers, or a 3-phase bank of single-phase units. |
| **Transmission lines and towers** | **Present** | `nuke_power_pole` 4 / 4,936, `nuke_power_pole_02` 10 / 7,980 — wood/lattice poles up to 33.8 m, around the site perimeter (X −150 … +162). |
| **Cabling** | **Present, very abundant** | `wires_001` **1,014 instances / 199,890 tris** and `wires_002` **448 / 113,060**. Category total **1,639 instances / 489 k tris**. Overhead spans, drops and hanging runs. |
| **Switchgear / MCC** | Partial | `nuke_electric_panel01` 4 cubicles. No switchgear room, no MCC line-up, no battery room, no inverters, no divisional 6.9 kV / 480 V distribution. |
| **Emergency power** | Absent | No EDGs (see §3.3), no station batteries, no combustion turbine generator. |

---

### 3.8 Balance of plant

| Item | State | Note |
|---|---|---|
| **Turbine-generator (1 HP + 3 LP, single shaft)** | **Absent** | No asset, no building, no pedestal, no lube-oil skid, no exciter. This must be an annotation proxy. |
| **Moisture separator reheaters** | **Absent** as MSRs; **stand-in available (good silhouette)** | `nuke_machinery_05` horizontal drums are the right shape and roughly the right slenderness for MSR shells; you would need six, at deck level flanking a turbine that does not exist. |
| **Main condenser** | **Absent** | Nothing under-floor, no condenser neck, no hotwell. |
| **Condenser air removal / SJAEs** | **Absent** | — |
| **Condensate pumps** | **Absent** | — |
| **Condensate demineralisers (full-flow polishing — BWR-specific)** | **Absent** | No vessel array; the map has nothing resembling a bank of deep-bed or powdex vessels. This is a *real* BWR fidelity miss and worth listing above the deaerator. |
| **Reactor feed pumps (3 × motor-driven with ASDs)** | **Absent** | The single `nuke_machinery_03` motor is the only driver in the map. |
| **LP / HP feedwater heater strings** | **Absent** | Again, `nuke_machinery_05` drums are the correct silhouette for closed FW heater shells and are the obvious proxy. |
| **Deaerator** | **Correctly absent — see §6.1** | An ABWR has none. |
| **Circulating water system** | **Stand-in available (good)** | The three ~1.5 m headers at X −35 … −8 (see §3.4) plus the river. No CW pumps, no intake screens, no trash racks, no discharge seal well. |
| **Intake / discharge structure** | **Stand-in available (weak)** | The low building the three headers terminate on, at ≈(−20, −6, 38). |
| **Cooling towers** | **Absent** | None. The river implies once-through, which is fine — Kashiwazaki-Kariwa is seawater once-through. |
| **Water storage** | **Present** | `nuke_silo_002` (§6) and the 5 × 24 m tank-farm tanks (§3.6). |

---

## 4. Instrumentation — full treatment

### 4.1 What the map genuinely models

The `control_room_displays_color` texture atlas
(`control_room_displays_color_psd_9adb2ad9.png`) is the
most information-dense asset in the map. It contains, all legible at close camera range:

- An **instrument cabinet with hinged doors** enclosing a large round moving-coil indicator (0–100),
  two horizontal **edgewise scale meters with red set-point pointers**, and blank digital windows.
- A **square-bezel round indicator, 0–150 scale**, with two integral indicator lamps — bench-board style.
- **Vertical edgewise strip meters**, one plain 0–100, one with a **coloured band (green/red) over
  10–60** — i.e. a bargraph with an operating band and an alarm band.
- **Selector switches**: a three-position 0 / I / II rotary with a pointer knob, and a plain pointer knob.
- Additional round dial gauges (0–100, two variants).
- A **6 × 4 backlit annunciator tile matrix** — the classic alarm window array, with tiles in several
  tints.
- **Illuminated pushbuttons** with red lamp caps, and a 4-lamp column.
- A **strip-chart recorder** with visible chart paper, gridlines, a red pen trace and a paper drive.

Additional instrumentation-bearing textures:

- `nuke_industrial_props_001_color` — MCB distribution board, relay/contactor cabinet with wiring, a
  6-pushbutton sub-panel with a round gauge, and a **legible circuit directory** reading:
  *Coolant Flow Control · Cooling Tank 1 2 3 · Core Temperature · Crane Control · Ventilation ·
  Primary Coolant · Secondary Coolant*, cross-referenced to *A-1…A-4, B-1…B-3, SUB D1…D-4, BKUP 1/2,
  REROUTE, RJB, PR1…PR3, JP-M, EQL, CXM*.
- `metal_pipe_002b_color` — **local pressure gauge (mbar / in.Hg compound scale) and bimetallic
  thermometer (0–40 °C)**, flanges, blind flange, bolted manway, bellows expansion joint.
- `nuke_office_desk_buttons_color` — desk console keyboard/button array with red/green/teal key blocks.
- `power_outlet_campground_color` — conduit, junction box, cable clamps.
- `nuke_electric_panel01/02_color` — switchgear cubicle door, small panel with indicator windows and
  lamp column.
- `signs_001_color` — radiological signage: **"Unsealed radionuclides", "Risk of contamination",
  "Radiation Controlled Area", "Caution — Radioactive waste"**, the trefoil, "High voltage",
  "Deep water", "Overhead crane", "Authorised personnel only", "Do not operate".

### 4.2 Where the instrumentation physically is

- **Main control room** (46 panel instances). Room **X −9.2 … +6.8, Z −18.6 … +6.4**, floor Y ≈ −10.5,
  ≈16 × 25 m. Layout: a long **benchboard console** down the west wall (X −9.2) with a **vertical panel
  array behind it**; further panel groups on the north wall (Z 6.4) and east wall (X 6.8); three
  **free-standing cabinet stacks** in the room; desks with CRT monitors; office chairs; whiteboards.
  [`evidence/ctrlroom_plan.png`](evidence/ctrlroom_plan.png),
  [`evidence/ctrlroom_in3.png`](evidence/ctrlroom_in3.png),
  [`evidence/ctrlroom_in2.png`](evidence/ctrlroom_in2.png).
- **Reactor-hall mimic banks** (5 instances, Y −14.6). Three wall-length arrays of instrument faces
  overlooking the pool hall: 14.2 m at Z 34.1, 14.2 m at Z 9.6, 11.0 m at X 24.5.
  [`evidence/crd_hall_iso.png`](evidence/crd_hall_iso.png). Visible in situ over the pool in
  [`evidence/rpvhead_y25.png`](evidence/rpvhead_y25.png) and [`evidence/poolplan.png`](evidence/poolplan.png).
- **Distributed field devices.** 72 conduit/junction boxes across **X −59 … +89**; 18 electrical panels
  across **X −64 … +68**; local gauges wherever `metal_pipe_002` is used (80 instances).

### 4.3 ABWR instrumentation checklist — item by item

| ABWR item | State | Comment |
|---|---|---|
| **SRNM** (10 fixed regenerative fission chambers; ABWR replaced SRM/IRM with their drive mechanisms) | **Absent** | No in-core instrument geometry, no under-vessel penetration, no SRNM drive housings. |
| **LPRM** (52 assemblies × 4 = 208 detectors) / **APRM** (4 divisions) / OPRM | **Absent** | No LPRM string penetrations in the vessel head, no APRM cabinets. |
| **ATIP** (3 machines, indexing mechanisms, purge) | **Absent** | No TIP room, no indexer, no drive tubing. |
| **Reactor water level** — narrow / wide / fuel-zone / shutdown range | **Absent** | No condensing chambers, no reference legs, no instrument nozzles, no ΔP transmitters, no instrument racks. **This is a notable miss for a post-Fukushima narrative:** the reference-leg flashing problem is one of the most narratable pieces of BWR instrumentation there is, and there is no geometry to point at. |
| **Reactor pressure** | **Absent** as a plant instrument | Local pipe gauges only (§4.1), and those are mbar/°C ranges — utility, not primary-circuit. |
| **Core flow derived from RIP performance** | **Absent** | No RIPs to derive it from. Worth saying on camera anyway: in an ABWR, core flow is inferred from the ten RIP characteristics and RIP ΔP — there are no jet pumps and therefore no jet-pump ΔP flow measurement. It is a clean differentiator you can state over the empty under-vessel space. |
| **Feedwater flow / main steam flow** | **Absent** | No flow elements, no venturis, no MSL flow restrictors. |
| **SSLC** (four divisions, 2-out-of-4, EMS fibre multiplexing) | **Absent** | The two `nuke_supercomputer` cabinets are the only logic-cabinet-shaped objects; they are unlabelled and there are two, not four divisions' worth. |
| **Digital MCR** — wide fixed display panel, flat-panel touchscreen consoles, automated plant startup | **Absent, and actively contradicted** | The map's MCR is an analogue benchboard with annunciator tiles and a chart recorder. Two design generations earlier. |
| **Main steam line radiation monitors** | **Absent** | No steam tunnel, no monitors. |
| **Process radiation monitoring** (offgas, stack, RCW/RSW, SFP) | **Absent** | No monitor skids, no sample panels, no shielded detector housings. |
| **Area radiation monitors** | **Absent** | Radiological *signage* exists (`signs_001`), instruments do not. Not one ARM head, local readout or dose-rate display anywhere. |
| **Containment atmosphere monitoring, H₂/O₂ analysers** | **Absent** | — |
| **Seismic instrumentation** | **Absent** | No accelerograph cabinets, no foundation-level instrument piers. |
| **Instrument racks, transmitters, impulse lines, root valves, manifolds** | **Absent** | This is the single biggest visual gap in the instrumentation story. A real plant is *full* of stainless-tube sensing lines in tube trays, 3- and 5-valve manifolds, condensing pots and local rack frames. The map has zero. What it has instead is panels on walls and gauges printed on pipes. |
| **Local gauge boards** | **Present in texture, absent as fixtures** | Gauges appear on the pipe kit and inside panel doors; there is no free-standing local gauge board. |
| **Conduit, junction boxes** | **Present** | 72 instances of `power_outlet_campground`. |
| **Cable trays** | **Uncertain** | Tray appears in the `nuke_industrial_props_001` texture atlas. I could not isolate tray geometry because the bake merges the kit into aggregates that no longer preserve sub-mesh names. **To resolve:** decompile `nuke_industrial_props_001.vmdl` in isolation, as was already done for `nuke_machinery_02/03/05_big`, and inspect the sub-mesh list. |

### 4.4 The correction the user should hear

The brief asked me to correct where warranted, and there are two corrections here.

**(a) The map's instrumentation is not "generic" — it is specifically period-analogue, and that is
useful.** The atlas contains an annunciator matrix and a strip-chart recorder. Those are not
placeholder art; someone drew a real control-room instrument set. If the video leans into "this is a
1970s-vintage control room" it becomes an *asset* rather than an error. If the video insists it is an
ABWR MCR, the same detail becomes the thing that convicts it.

**(b) The circuit directory says "Primary Coolant" and "Secondary Coolant".** That is a two-circuit
plant. In a BWR there is no secondary coolant — steam raised in the core goes straight to the turbine,
the feedwater train is part of the reactor coolant boundary, and the condensate polishers exist
precisely *because* the whole condensate stream is reactor water. If you are narrating a BWR, either
avoid that panel or call it out deliberately as the map getting it wrong.

---

## 5. Vessels and tanks

### 5.1 Inventory

| Asset | n | Size | Location | Reads as | State |
|---|---|---|---|---|---|
| `nuke_silo_001b` (large shell) | 5 frags | 35.2 × 41.2 × 35.7 m | X ≈ 17, Z ≈ −6 | Spherical steel containment / gas holder | **Present** (as containment shell) |
| `nuke_silo_001` (small shell) | 19 frags | ≈22 × 31 × 22 m | X ≈ 60, Z ≈ 24 | Same family, half scale | **Present** |
| `nuke_silo_002` | 17 frags / 12,126 tris | **11.2 dia × 10.1 m tall** | bbox `[−3.6, −10.6, 36.6] .. [7.6, −0.4, 47.8]` | **Bolted-plate vertical water storage tank** — vertical stiffener strakes, shell manway, two nozzle stubs, lifting lugs, top guardrail cage | **Present** |
| `medium_silo` | 8 / 15,408 tris | 5.3 dia × 7.1 m each | X 107–129, Z −20…+23 | **Dry storage casks on an ISFSI pad** | **Present** |
| `nuke_machinery_01` (large) | 5 / ≈2,236 each | **24 dia × 11 m** | Z 116–147, X −134…+132 | Domed-top bulk water storage tanks with springline handrail | **Present** |
| `nuke_machinery_01` (small) | 4 | 3.7 dia × 5.4 m | (3.5, +4.5, −11.6 / −2.7) | Small vertical process tank | **Present** |
| `nuke_machinery_05` | 6 / 30,080 tris | 2 × ~19 m and 2 × ~12.4 m, ≈3 m dia | see §3.6 | **Horizontal pressure vessels** with dished heads, saddles, manway | **Present** |
| Bell-shaped flanged body next to `nuke_silo_002` | 1 | ≈1.5 m | ≈(−2.5, −5.5, 42) | Large **basket strainer / suction pot / relief-valve body** | **Present** |
| `nuke_reactor_vessel_head` | 1 | 6.6 dia × 3.9 m | (−3.6, −26.8, 22.8) | RPV closure head, submerged | **Present** |

Evidence: [`evidence/silo002_y210.png`](evidence/silo002_y210.png),
[`evidence/strainer_y290.png`](evidence/strainer_y290.png),
[`evidence/mediumsilo.png`](evidence/mediumsilo.png),
[`evidence/m01_tank_y30.png`](evidence/m01_tank_y30.png),
[`evidence/m05_a_y40.png`](evidence/m05_a_y40.png),
[`evidence/solo_vessel_iso.png`](evidence/solo_vessel_iso.png).

### 5.2 Mapping map vessels onto ABWR vessels

| ABWR vessel | Best available map asset | Match quality |
|---|---|---|
| **CST** (condensate storage tank, ~2,000 m³, RCIC/HPCF preferred suction) | `nuke_silo_002`, 11 m × 10 m ≈ 970 m³ | **Good.** Right form, right order of magnitude, right kind of place (outdoors, adjacent to the plant, piped in). This is the strongest single vessel match in the map. |
| **Demineralised / raw water storage** | The five 24 m × 11 m `nuke_machinery_01` tanks (≈5,000 m³ each) | **Good**, but they sit south of the river outside the site fence at Z ≈ 116–147, which is scenery. Usable only in wide shots. |
| **Suppression pool** | none | **Absent.** No annular or toroidal water volume of any kind. |
| **RHR heat exchangers** (3, vertical U-tube in ABWR) | `nuke_machinery_05` drums | **Poor–fair.** ABWR RHR heat exchangers are *vertical*; the map's drums are horizontal. Silhouette works at distance only. |
| **SLC storage tank** (~15 m³, sodium pentaborate) | none of the right size | **Absent.** |
| **RPV** | none | **Absent** (head only). |
| **Feedwater heaters / MSRs** | `nuke_machinery_05` drums | **Fair.** Correct horizontal shell-and-saddle form; wrong count, wrong rooms. |
| **Deaerator** | — | **Not applicable.** See §6.1. |
| **Spent fuel casks** | `medium_silo` × 8 | **Good.** |

### 5.3 On the deaerator, directly

**An ABWR does not have a deaerator, and neither does any other BWR.** So the correct answer to "is the
deaerator present or absent?" is: *the question does not apply to this plant type, and the map's lack of
one is not an error.*

Why BWRs have no deaerating feedwater heater:

1. **The main condenser is the deaerator.** It runs under deep vacuum with continuous non-condensable
   removal by steam-jet air ejectors (or vacuum pumps) and a deaerating hotwell arrangement. It gets
   dissolved oxygen down to single-digit ppb before the condensate pumps, which is all the feed train
   needs.
2. **Feedwater is reactor coolant.** In a BWR the condensate/feedwater train is inside the reactor
   coolant circuit — there is no steam generator to divide "primary" from "secondary". Everything in
   that train carries activated corrosion products, N-16 during operation, and traces of fission
   products from any fuel defect. An **open, direct-contact** heater with a large storage volume would
   be a big, vented, radioactive water inventory sitting in the turbine building. Every BWR feedwater
   heater is therefore a **closed shell-and-tube** unit.
3. **Full-flow condensate polishing forbids it.** BWRs polish the *entire* condensate stream through
   deep-bed or powdered-resin demineralisers immediately downstream of the condensate pumps. A
   deaerator with a large storage tank downstream would introduce an unpolished, unmonitored
   inventory and destroy the control the polishers give over feedwater chemistry.
4. **Oxygen control in a BWR is deliberate, not a deaeration problem.** Radiolysis in the core produces
   oxygen and hydrogen continuously; feedwater oxygen is *managed* (normal water chemistry runs with
   dissolved O₂ present; hydrogen water chemistry and noble-metal chemical addition are the tools for
   IGSCC mitigation). Scrubbing oxygen out with a deaerator would neither be possible nor desirable.

**Corollary — what you should look for instead.** The correct BOP checklist item for a BWR is not a
deaerator, it is the **condensate demineraliser / full-flow polishing plant**, which is BWR-specific and
which a PWR does not have in that form. **That is absent from the map** and is a more defensible thing
to flag than the deaerator. Likewise the **steam-jet air ejectors and the offgas train** downstream of
them — the BWR condenser air-removal path carries radioactive noble gases and feeds recombiners and
charcoal delay beds, which is another genuinely BWR-only piece of plant, and is also absent.

Note also that "deaerator" is not universally absent from nuclear plants — several PWR and VVER designs
do use deaerating feedwater heaters, because on those plants the feed train is on the *secondary* side
and is not radioactive. If you say "nuclear plants don't have deaerators" you will be wrong. Say
"**BWRs** don't have deaerators, because in a BWR the feedwater is the reactor coolant."

---

## 6. Annotation proxies that would have to be added, in priority order

Ordered by how badly the video breaks without them.

1. **RCCV.** Replace or re-frame the containment. Either (a) build a rectangular reactor-building box
   with a flat top slab around the pool hall and label the containment as an RCCV inside it, or
   (b) accept the spheres and narrate them as "the containment shell, which on a real ABWR is a
   concrete cylinder inside the building — here it is drawn as a sphere". Option (b) costs nothing and
   is honest; option (a) is a large modelling job. **Do not silently call the sphere an RCCV.**
2. **The turbine island in its entirety.** TG (1 HP + 3 LP on one shaft), generator, exciter,
   6 MSRs, condenser, SJAEs, condensate pumps, **condensate polishers**, 3 motor-driven RFPs with ASDs,
   LP and HP heater strings. Best placement: the currently featureless building block west of the
   containment (**X ≈ −60 … −10, Z ≈ 20 … 55**) — it already has the three large-bore headers running
   into it, which makes it read as the CW-served end of the plant. Proxy geometry can reuse
   `nuke_machinery_05` for MSRs and heaters and `nuke_machinery_02` for the TG hood.
3. **RPV + RIPs + FMCRDs as a cutaway.** The most ABWR-specific thing you can show. A single labelled
   cutaway — vessel, 10 RIP casings ringing the bottom head, FMCRD housings below, no external recirc
   loops — sells the design in one shot and needs no map geometry at all if it is an overlay. Anchor it
   over the pool at (0, −24, 25) where the head prop already sits.
4. **Four main steam lines and the steam tunnel.** Route four labelled lines horizontally out of the
   containment at operating-deck level (Y ≈ −13) eastward into the turbine block, with MSIV pairs
   marked. Do **not** reuse the crown risers; label those separately or leave them unlabelled.
5. **Suppression pool + horizontal vents.** Even a schematic annulus at the base of the containment,
   labelled, prevents the "where does the SRV discharge go?" question.
6. **Three-division safety trains.** Three EDG blocks, three RHR pump rooms, three RCW/RSW trains, and
   divisional colour on cable runs. The map already has 1,639 electrical instances and 72 junction
   boxes; recolouring a subset red/green/blue by division would be cheap and would read instantly.
7. **Instrument racks and sensing lines.** A dozen local instrument racks with transmitters and impulse
   tubing, placed on the existing catwalks, would do more for "this looks like a real plant" than any
   other single addition. There are 499 access-steel instances to hang them from.
8. **RB stack / plant vent, SGTS filter train, offgas plant.** A 100 m stack changes the skyline and
   makes the site read as nuclear from every wide shot.
9. **Reactor building crane over the refuelling floor**, and a refuelling machine bridge over the pool.
   The `nuke_cargo_crane` mesh can be reused indoors at Y ≈ −12 spanning the pool hall.
10. **Radiation monitors.** ARM heads with local readouts on the pool hall walls, a stack monitor skid,
    MSL radiation monitors in the steam tunnel. Small props, big credibility.

---

## 7. Narration notes and caveats

Things to say on camera so the video survives an informed viewer.

- **Get ahead of the dome.** Say early: *"The building you are looking at is a spherical containment
  shell. A real ABWR does not look like this — its containment is a reinforced-concrete cylinder with a
  flat top slab, buried inside a rectangular reactor building. Kashiwazaki-Kariwa 6 and 7 look like
  warehouses. We are using the sphere as a stand-in."* This converts your biggest liability into a
  teaching moment in one sentence.
- **Use the absence of recirculation loops as a positive.** *"Notice what is not here: no external
  recirculation loops, no jet pump risers, no big primary piping below the core. That is genuinely how
  an ABWR is built — the ten reactor internal pumps sit in the bottom head of the vessel itself."* You
  are describing correct ABWR architecture while standing in an empty room, which is defensible.
- **The RPV head is a refuelling-outage shot, not an operating shot.** The head is on the pool floor
  under water. Narrate it as a plant in outage: head laid down, fuel in the racks, refuelling tools on
  the wall. Everything else on the site then makes sense as a shutdown state.
- **Do not claim the control room is an ABWR MCR.** Say: *"This is an analogue benchboard control room —
  annunciator tiles, edgewise meters, a chart recorder. An ABWR's control room is fully digital: a wide
  display panel and flat-panel consoles, with SSLC in four divisions behind it and automated startup.
  What you are seeing is two generations older."*
- **On the deaerator, be precise.** *"People ask where the deaerator is. A BWR doesn't have one. The
  condenser does the deaeration, under vacuum, with the air ejectors — and because feedwater in a BWR
  is reactor coolant, you would never put an open, vented heater with a big water inventory in that
  train. Every BWR feedwater heater is a closed shell-and-tube unit. The thing you should be looking
  for instead is the condensate polishing plant, and that isn't here either."*
- **Flag the panel that says "Secondary Coolant"** if you show it, or avoid it. It is a PWR label on a
  BWR set.
- **The three big pipes by the river are your best real-plant beat.** Three large-bore headers running
  to a structure on a 294 m river is a genuine circulating-water composition. Use it.
- **The cask pad is your second best.** Eight vertical overpacks on a marked concrete pad with spare
  positions is exactly what an ISFSI looks like. It is the one thing in the map an industry viewer will
  nod at without qualification.
- **Do not call `gas_meter` an instrument.** Its texture is a domestic diaphragm gas meter with a
  cubic-metre odometer. If it appears on camera, it is the site's natural-gas service, not process
  instrumentation.
- **Scale caveats worth pre-empting:** fuel pool is 6.9 m deep (real ≈11–12 m); rack cell pitch scales
  to ~1.7 m (real ≈15 cm); casks are 5.3 m diameter (real ≈3.4 m); the "containment" is 35 m across
  (an ABWR RCCV is ~29 m inside diameter, so that one is actually close).
- **Two containments = two units is not defensible.** They are 35 m and 22 m across. No site pairs
  containments of such different size. Either shoot them separately, or call the small one an
  auxiliary/gas-storage sphere.

---

## 8. Open items and how to close them

| Uncertainty | How to resolve |
|---|---|
| Whether dedicated **cable-tray geometry** exists (tray appears in the `nuke_industrial_props_001` texture) | Decompile `nuke_industrial_props_001.vmdl` in isolation, as `nuke_machinery_02/03/05_big` already were, and list its sub-meshes. The map bake merges the kit into aggregates that no longer carry sub-mesh names. |
| Exact composition of the three-pipe manifold with in-line bodies at the west building face (≈ X −33, Y −7, Z 28) — possible valve or pump-discharge geometry | Render `metal_pipe_003` in isolation at close range; that material has 23 instances / 19,244 tris and is the least characterised of the three pipe kits. |
| Whether any `metal_pipe_002` instance actually *shows* the gauge UV region on camera | Walk the 80 `metal_pipe_002` instances and check UV coverage; the atlas proves the art exists but not that it is placed where a camera can see it. |
| Whether the four flanged standpipes at (8–13, −20, 40–50) are worth reusing as a CRD-grid proxy | Close render from below; they are currently the only under-floor standpipe cluster in the map. |

---

## 9. Appendix — headline counts

Category totals as the viewer classifies them (`nukeplant.exe --stats` against the pre-rebuild binary):

| category | instances | triangles | share | extent (m) |
|---|---|---|---|---|
| pipe | 1,927 | 504,556 | 14.2 % | 178 × 66 × 98 |
| duct | 364 | 149,256 | 4.2 % | 199 × 31 × 90 |
| vessel | 51 | 60,440 | 1.7 % | 136 × 72 × 72 |
| instrument | 111 | 190,700 | 5.4 % | 267 × 27 × 170 |
| electrical | 1,639 | 489,016 | 13.7 % | 318 × 45 × 158 |
| access | 499 | 415,454 | 11.7 % | 290 × 47 × 149 |
| lighting | 603 | 255,966 | 7.2 % | 283 × 35 × 137 |
| other | 3,431 | 1,491,384 | 41.9 % | 321 × 71 × 207 |
| **total** | **8,625** | **3,556,772** | | |

Two classification notes for the project, not the video:

- **`gas_meter` should not be in `instrument`.** It is a domestic gas service meter.
- **`nuke_machinery_*` should not be in `instrument`.** Those four assets are the map's only pressure
  vessels, storage tanks and rotating machinery — they belong in `vessel` (or a new `equipment`
  category) alongside the silos. As it stands the `instrument` category's 190,700 triangles are 78 %
  machinery, which makes the `--solo instrument` view misleading
  ([`evidence/instr_plan.png`](evidence/instr_plan.png) is dominated by drums, not panels).

Key per-asset counts referenced above:

```
1822  413,584  metal_pipe_001            295  133,322  airduct_hvac_001
  80   69,314  metal_pipe_002            25    2,896  nuke_ventilation_exhaust_small
  23   19,244  metal_pipe_003            19    2,858  nuke_ventilation_exhaust_002
1014  199,890  wires_001                 15    4,196  nuke_ventilation_exhaust
 448  113,060  wires_002                  5   12,032  nuke_silo_001b   (large shell)
  51   69,226  control_room_displays     19   13,760  nuke_silo_001    (small shell)
  72   30,240  power_outlet_campground   17   12,126  nuke_silo_002    (bolted tank)
  17   46,662  current_transformer        8   15,408  medium_silo      (casks)
   8   33,722  nuke_circuit_breaker      15   28,474  nuke_machinery_01
  36   32,322  transformer_wires          2    9,952  nuke_machinery_02
  29   20,190  substation_support         1    8,552  nuke_machinery_03
   4    5,558  substation_transformer     6   30,080  nuke_machinery_05
  18   20,094  nuke_electric_panel01/02   1    3,014  nuke_reactor_vessel_head
  32    6,784  sprinkler_001              1    4,100  nuke_spent_fuel_racks
 233    9,646  signs_001 + area signs     7      120  steam_001_additive
```
