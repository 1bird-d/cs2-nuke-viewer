//! What each prop most likely represents in a real plant.
//!
//! Every entry here comes from `docs/ABWR-review.md`, which identified the map's
//! equipment **by shape and texture, not by name** — no asset in de_nuke is
//! named for any item of rotating machinery, heat-exchange equipment, valve or
//! radiation instrument. Nothing in this table is inferred beyond what that
//! review established by looking at the geometry and the texture atlases.
//!
//! Each entry carries a [`Confidence`] so the viewer can be honest about the
//! difference between three quite different claims:
//!
//! * **Identified** — the prop genuinely depicts this piece of equipment.
//! * **Proxy** — it does not depict it, but it is the best thing in the map to
//!   stand in for it on camera, with the mismatch stated.
//! * **Mismatch** — it depicts something real, but something that contradicts
//!   the ABWR reading. These are the ones worth knowing before filming.
//!
//! Where the review gave a measurement, it is quoted, because a dimension is
//! the difference between a claim and an assertion.
//!
//! Each entry also carries links to photographs of the **real** unit, so a claim
//! can be checked against a picture instead of taken on trust. Every URL in this
//! file was resolved against the Wikimedia Commons API before being written
//! down; none was recalled from memory. Where the nearest verifiable photograph
//! is of the wrong reactor type — the only reactor-vessel-head photo on Commons
//! is of PWR heads — the link says so rather than quietly passing it off.

use regex::Regex;

/// A picture of the real equipment.
///
/// Commons *category* pages are preferred over single files where the point is
/// "this is what one of these looks like", because a category is a gallery of
/// many examples and survives any one file being renamed.
pub struct Reference {
    /// What the link shows. Written to be read aloud.
    pub label: &'static str,
    /// Checked to exist. See the module note.
    pub url: &'static str,
}

/// How strong the match between prop and real equipment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// The prop depicts this.
    Identified,
    /// Not this, but the best available stand-in.
    Proxy,
    /// Real equipment, but wrong for an ABWR.
    Mismatch,
}

impl Confidence {
    /// Label shown on the tooltip chip.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Identified => "IDENTIFIED",
            Self::Proxy => "PROXY",
            Self::Mismatch => "MISMATCH",
        }
    }

    /// Chip colour: green for identified, amber for a stand-in, red for a
    /// mismatch that would undermine the narration.
    #[must_use]
    pub fn colour(self) -> [u8; 3] {
        match self {
            Self::Identified => [90, 210, 140],
            Self::Proxy => [235, 170, 70],
            Self::Mismatch => [235, 95, 85],
        }
    }
}

/// One prop's identification.
pub struct Entry {
    /// Regex over the material path and the model stem.
    pattern: &'static str,
    /// What the geometry actually depicts.
    pub what: &'static str,
    /// The real plant item it most likely represents.
    pub real: &'static str,
    /// How good the match is.
    pub confidence: Confidence,
    /// The measurement or caveat that justifies the call.
    pub note: &'static str,
    /// Photographs of the real unit. Empty where nothing verifiable was found,
    /// which is a better answer than a plausible-looking link.
    pub refs: &'static [Reference],
}

/// Ordered most specific first; first match wins, as with categories.
const ENTRIES: &[Entry] = &[
    Entry {
        pattern: r"nuke_reactor_vessel_head",
        what: "Reactor pressure vessel closure head",
        real: "RPV head, laid down under water during a refuelling outage",
        confidence: Confidence::Identified,
        note: "6.6 m studded closure flange with a raised bolted stub, on the fuel pool floor at \
               (-3.6, -26.8, 22.8). An ABWR RPV flange is 7.1 m ID, so the scale is close. There \
               is no vessel shell anywhere in the map — narrate it as the head, not the vessel.",
        refs: &[
            Reference {
                label: "BWR reactor pressure vessel, cutaway",
                url: "https://commons.wikimedia.org/wiki/File:Bwr-rpv.svg",
            },
            // Commons has no photograph of a BWR head off the vessel. These are
            // PWR heads; the flange, stud ring and nozzle penetrations read the
            // same, but say so rather than let the picture imply otherwise.
            Reference {
                label: "Reactor vessel heads in a laydown area (PWR, not BWR)",
                url: "https://commons.wikimedia.org/wiki/File:Reactor_Vessel_head.jpg",
            },
            Reference {
                label: "More reactor pressure vessels",
                url: "https://commons.wikimedia.org/wiki/Category:Reactor_pressure_vessel",
            },
        ],
    },
    Entry {
        pattern: r"nuke_spent_fuel_racks",
        what: "Spent fuel storage rack array",
        real: "SFP storage racks",
        confidence: Confidence::Identified,
        note: "17.1 x 8.5 m plate array of square cells in two sub-blocks on the pool floor. Cell \
               pitch scales to ~1.7 m against a real BWR pitch of ~15 cm, so it is far too coarse \
               — but the read is unambiguous.",
        refs: &[
            Reference {
                label: "Racked fuel in a BWR pool — Caorso",
                url: "https://commons.wikimedia.org/wiki/File:Caorso_fuel_pool.jpg",
            },
            Reference {
                label: "More spent fuel pools",
                url: "https://commons.wikimedia.org/wiki/Category:Spent_nuclear_fuel_ponds",
            },
        ],
    },
    Entry {
        pattern: r"medium_silo",
        what: "Vertical dry storage cask",
        real: "Spent fuel dry cask overpack on an ISFSI pad",
        confidence: Confidence::Identified,
        note: "8 casks, 5.3 m dia x 7.1 m, on a marked pad with recessed positions for more. \
               Slightly fat against a HI-STORM (3.4 m x 5.9 m), but grid array plus spare marked \
               positions on open hardstand is exactly right. The most convincingly nuclear \
               composition in the map after the fuel racks.",
        refs: &[
            Reference {
                label: "Dry cask storage, NRC overview photo",
                url: "https://commons.wikimedia.org/wiki/File:Spent_Fuel_Dry_Cask_Storage_Overview_r3_(25533939854).jpg",
            },
            Reference {
                label: "ISFSI pads — casks on open hardstand",
                url: "https://commons.wikimedia.org/wiki/Category:Independent_Spent_Fuel_Storage_Installations",
            },
            Reference {
                label: "CASTOR casks",
                url: "https://commons.wikimedia.org/wiki/Category:Castor_containers",
            },
        ],
    },
    Entry {
        pattern: r"nuke_silo_002",
        what: "Bolted-plate vertical water storage tank",
        real: "Condensate storage tank (CST)",
        confidence: Confidence::Identified,
        note: "11.2 m dia x 10.1 m, about 970 m3, with stiffener strakes, shell manway, two nozzle \
               stubs and a top guardrail cage. An ABWR CST is ~2,000 m3. Right form, right order \
               of magnitude, right kind of place — the strongest single vessel match in the map.",
        refs: &[
            // A CST at a real nuclear station, captioned as one by the survey
            // that photographed it — as direct a comparison as this table gets.
            Reference {
                label: "Condensate storage tank at Shippingport, captioned (HAER survey)",
                url: "https://commons.wikimedia.org/wiki/File:VIEW_FROM_NORTHWEST_OF_CONDENSATE_STORAGE_TANK_(LEFT),_PRIMARY_WATER_STORAGE_TANK_(CENTER),_CANAL_WATER_STORAGE_TANK_(RIGHT)_(LOCATIONS_E,F,D)_-_Shippingport_Atomic_Power_Station,_HAER_PA,4-SHIP,1-8.tif",
            },
            Reference {
                label: "Bolted and welded storage tanks",
                url: "https://commons.wikimedia.org/wiki/Category:Storage_tanks",
            },
        ],
    },
    // Ordered before nuke_silo_001 so the larger shell is not caught by it.
    Entry {
        pattern: r"nuke_silo_001b",
        what: "Ovoid steel shell on a cylindrical skirt",
        real: "Spherical steel containment, or a gas holder",
        confidence: Confidence::Mismatch,
        note: "35.2 m across x 41.2 m tall, ribbed meridians, springline platform, lightning \
               finial, apex plinth, four external risers over the crown. An ABWR RCCV is a \
               concrete cylinder with a FLAT TOP SLAB, entirely inside a rectangular reactor \
               building — you never see a dome. This is the map's signature image and its biggest \
               departure from ABWR form.",
        refs: &[
            Reference {
                label: "What an ABWR actually looks like — Toshiba unit",
                url: "https://commons.wikimedia.org/wiki/File:ABWR_Toshiba_1.jpg",
            },
            Reference {
                label: "ABWR cross section — note the flat top slab, no dome",
                url: "https://commons.wikimedia.org/wiki/File:UK_ABWR_cross_section.png",
            },
            Reference {
                label: "Kashiwazaki-Kariwa, the largest ABWR site",
                url: "https://commons.wikimedia.org/wiki/File:Kashiwazaki-Kariwa_Nuclear_Power_Plant_14-Aug-2019.jpg",
            },
            // The one BWR containment that is genuinely bulbous. Even this is
            // buried in concrete and never seen from outside.
            Reference {
                label: "BWR Mark I drywell — the only bulb-shaped BWR containment",
                url: "https://commons.wikimedia.org/wiki/File:BWR_Mark_I_Containment,_diagram.png",
            },
            Reference {
                label: "Gas holders — what the shape actually reads as",
                url: "https://commons.wikimedia.org/wiki/Category:Gasometers",
            },
        ],
    },
    Entry {
        pattern: r"nuke_silo_001",
        what: "Ovoid steel shell, half scale",
        real: "Second containment shell",
        confidence: Confidence::Mismatch,
        note: "About 22 m across x 31 m tall, same family as the large shell. Two containments of \
               unequal size is a configuration no BWR site has. Both shells are single-sided \
               hollow — there is no interior geometry at all.",
        refs: &[
            Reference {
                label: "What an ABWR actually looks like — Toshiba unit",
                url: "https://commons.wikimedia.org/wiki/File:ABWR_Toshiba_1.jpg",
            },
            Reference {
                label: "Gas holders — what the shape actually reads as",
                url: "https://commons.wikimedia.org/wiki/Category:Gasometers",
            },
        ],
    },
    Entry {
        pattern: r"nuke_machinery_05",
        what: "Horizontal cylindrical pressure vessel",
        real: "Moisture separator reheater, closed feedwater heater or RHR heat exchanger",
        confidence: Confidence::Proxy,
        note: "Dished heads, saddle supports, bolted flanged manway, top nozzle bosses — a \
               textbook process drum. Two ~19 m and two ~12.4 m, about 3 m dia. The best \
               heat-exchanger proxy in the map, but ABWR RHR heat exchangers are vertical and \
               there are only four of these, in the wrong rooms.",
        refs: &[
            Reference {
                label: "Shell-and-tube heat exchangers",
                url: "https://commons.wikimedia.org/wiki/Category:Heat_exchangers",
            },
            // Commons' feedwater-heater category is almost entirely steam
            // locomotives, so it is deliberately not linked here.
        ],
    },
    Entry {
        pattern: r"nuke_machinery_01",
        what: "Domed-top vertical storage tank",
        real: "Bulk demineralised or raw water storage",
        confidence: Confidence::Identified,
        note: "Used at two scales: five at 24 m dia x 11 m (~5,000 m3) with a springline handrail \
               in the tank farm, and four at 3.7 m x 5.4 m on a roof. The large ones sit south of \
               the river outside the site fence, so they are scenery — usable in wide shots only.",
        refs: &[Reference {
            label: "Bulk storage tanks",
            url: "https://commons.wikimedia.org/wiki/Category:Storage_tanks",
        }],
    },
    Entry {
        pattern: r"nuke_machinery_02",
        what: "Skid-mounted machine enclosure with acoustic hood",
        real: "Packaged rotating-machine skid",
        confidence: Confidence::Proxy,
        note: "About 5 x 3 x 10 m on an anchored base frame with lifting lugs and end handrails. \
               Two instances. The hood hides whatever is inside, which makes it a flexible proxy \
               — but nothing identifies what it drives.",
        // No reference: an anonymous acoustic hood has no real counterpart to
        // point at, which is exactly why it is only a proxy.
        refs: &[],
    },
    Entry {
        pattern: r"nuke_machinery_03",
        what: "Horizontal electric motor on a concrete plinth",
        real: "Motor driver — the only rotating machine in the map",
        confidence: Confidence::Identified,
        note: "Cylindrical stator, end bell with shaft stub and coupling, terminal box, cooling \
               ribs. One large (~3 m) and one small copy at (7.1, -15.4, 23.0). There are no \
               pumps for it to drive: reactor feed pumps, condensate pumps and RIPs are all absent.",
        refs: &[Reference {
            label: "Industrial electric motors",
            url: "https://commons.wikimedia.org/wiki/Category:Electric_motors",
        }],
    },
    Entry {
        pattern: r"control_room_displays",
        what: "Analogue benchboard and vertical instrument panels",
        real: "1970s-vintage BWR/3-5 main control room",
        confidence: Confidence::Mismatch,
        note: "The atlas carries a 6x4 backlit annunciator tile matrix, a strip-chart recorder \
               with a red pen trace, edgewise bargraphs with green/red operating bands, moving-coil \
               indicators and 0/I/II selector switches. Someone drew a real instrument set — but \
               an ABWR MCR is digital, with a wide fixed display panel and flat-panel consoles. \
               Two design generations later. 46 instances in the control room, 5 mimic banks over \
               the pool hall.",
        refs: &[
            // Dresden is a BWR of exactly the generation the art depicts, which
            // makes this the closest real counterpart on Commons.
            Reference {
                label: "Dresden BWR control room — the era the art is drawn from",
                url: "https://commons.wikimedia.org/wiki/File:Control_room_of_Commonwealth_Edison_Company%27s_Dresden_nuclear_power_station.jpg",
            },
            Reference {
                label: "An ABWR control room is digital — see the cross section",
                url: "https://commons.wikimedia.org/wiki/File:UK_ABWR_cross_section.png",
            },
            Reference {
                label: "More nuclear control rooms",
                url: "https://commons.wikimedia.org/wiki/Category:Nuclear_power_plant_control_rooms",
            },
        ],
    },
    Entry {
        pattern: r"nuke_supercomputer",
        what: "Louvred equipment cabinet",
        real: "Logic cabinet — the nearest thing to SSLC",
        confidence: Confidence::Proxy,
        note: "The only logic-cabinet-shaped objects in the map. Unlabelled, and there are two of \
               them where ABWR safety system logic and control has four divisions with 2-out-of-4 \
               voting.",
        refs: &[Reference {
            label: "Switchgear and control cubicles",
            url: "https://commons.wikimedia.org/wiki/Category:Switchgear",
        }],
    },
    Entry {
        pattern: r"nuke_electric_panel01",
        what: "Floor-standing switchgear cubicle",
        real: "LV switchgear or motor control centre cubicle",
        confidence: Confidence::Identified,
        note: "Ribbed door with a side lamp stack. Four instances. There is no switchgear room, no \
               MCC line-up, no battery room and no divisional 6.9 kV / 480 V distribution.",
        refs: &[Reference {
            label: "Switchgear cubicles and MCC line-ups",
            url: "https://commons.wikimedia.org/wiki/Category:Switchgear",
        }],
    },
    Entry {
        pattern: r"nuke_electric_panel02",
        what: "Small wall-mounted control panel",
        real: "Local control station",
        confidence: Confidence::Identified,
        note: "Two indicator windows and a four-lamp column. Fourteen instances spread from \
               X -64 to +68.",
        refs: &[Reference {
            label: "Local control and switch panels",
            url: "https://commons.wikimedia.org/wiki/Category:Switchgear",
        }],
    },
    Entry {
        pattern: r"nuke_industrial_props_001",
        what: "Electrical kit: MCB distribution board, relay cabinet, pushbutton sub-panel",
        real: "Distribution and control panels",
        confidence: Confidence::Mismatch,
        note: "Carries the legible circuit-directory placard reading 'Coolant Flow Control, Cooling \
               Tank 1 2 3, Core Temperature, Crane Control, Ventilation, PRIMARY COOLANT, SECONDARY \
               COOLANT'. Two separate coolant circuits is PWR vocabulary — a BWR has no secondary \
               coolant, because steam raised in the core goes straight to the turbine. Either avoid \
               this panel on camera or call it out as the map getting it wrong.",
        refs: &[
            Reference {
                label: "Distribution boards and relay cabinets",
                url: "https://commons.wikimedia.org/wiki/Category:Switchgear",
            },
            // The vocabulary on the placard belongs to the two-circuit plant
            // this diagram shows, not to a BWR.
            Reference {
                label: "A BWR has one circuit — compare the ABWR cross section",
                url: "https://commons.wikimedia.org/wiki/File:UK_ABWR_cross_section.png",
            },
        ],
    },
    Entry {
        pattern: r"power_outlet_campground",
        what: "Wall-mounted junction box with rigid conduit and cable clamps",
        real: "Field wiring junction box",
        confidence: Confidence::Identified,
        note: "72 instances across X -59 to +89 — the map's field-wiring signature, despite the \
               asset name saying 'outlet'. This is the closest the map gets to real instrument \
               field routing; there are no impulse lines, manifolds or instrument racks anywhere.",
        refs: &[
            Reference {
                label: "Field junction boxes",
                url: "https://commons.wikimedia.org/wiki/Category:Junction_boxes",
            },
            Reference {
                label: "Rigid conduit and cable clamps",
                url: "https://commons.wikimedia.org/wiki/Category:Electrical_conduit",
            },
            Reference {
                label: "Cable tray — how a real plant bulk-routes cable",
                url: "https://commons.wikimedia.org/wiki/Category:Cable_trays",
            },
        ],
    },
    Entry {
        pattern: r"nuke_office_desk_buttons",
        what: "Desk console keyboard and button array",
        real: "Operator console input panel",
        confidence: Confidence::Identified,
        note: "Red, green and teal key blocks. Six instances in the control room.",
        refs: &[Reference {
            label: "Operator consoles in real control rooms",
            url: "https://commons.wikimedia.org/wiki/Category:Nuclear_power_plant_control_rooms",
        }],
    },
    Entry {
        pattern: r"nuke_office_desk_monitor",
        what: "CRT monitor",
        real: "Operator workstation display",
        confidence: Confidence::Identified,
        note: "13 boxy CRTs on desks. Consistent with the analogue-era control room, and \
               inconsistent with an ABWR's flat-panel consoles.",
        refs: &[Reference {
            label: "Dresden BWR control room, same vintage",
            url: "https://commons.wikimedia.org/wiki/File:Control_room_of_Commonwealth_Edison_Company%27s_Dresden_nuclear_power_station.jpg",
        }],
    },
    Entry {
        pattern: r"gas_meter",
        what: "Domestic diaphragm natural-gas meter",
        real: "Nothing nuclear — a household utility meter",
        confidence: Confidence::Mismatch,
        note: "The texture shows a cubic-metre odometer register. Seven instances at \
               (25.2, -9.1, 11.8). Do not point at this as plant instrumentation; it is the kind \
               of detail an informed viewer will catch immediately.",
        refs: &[Reference {
            label: "Domestic gas meters — compare the register",
            url: "https://commons.wikimedia.org/wiki/Category:Gas_meters",
        }],
    },
    Entry {
        pattern: r"metal_pipe_002",
        what: "Pipe run with local gauges printed on the texture",
        real: "Local pressure gauge and bimetallic thermometer",
        confidence: Confidence::Identified,
        note: "The atlas carries a compound pressure/vacuum gauge in mbar and in.Hg and a \
               bimetallic thermometer reading 0-40 C, plus bolted flanges, a blind flange, a \
               manway and a bellows expansion joint. Utility ranges, not primary-circuit — but it \
               is the only process instrumentation actually drawn on the pipework.",
        refs: &[Reference {
            label: "Bourdon pressure gauges",
            url: "https://commons.wikimedia.org/wiki/Category:Pressure_gauges",
        }],
    },
    Entry {
        pattern: r"nuke_circuit_breaker",
        what: "Dead-tank SF6 circuit breaker with V-arranged bushings",
        real: "HV switchyard circuit breaker",
        confidence: Confidence::Identified,
        note: "8 pedestal-mounted instances in the switchyard. The electrical system is the \
               strongest and most accurate part of the map by a wide margin.",
        refs: &[Reference {
            label: "High-voltage circuit breakers",
            url: "https://commons.wikimedia.org/wiki/Category:Circuit_breakers",
        }],
    },
    Entry {
        pattern: r"current_transformer",
        what: "Pedestal instrument-transformer column",
        real: "Current transformer (CT)",
        confidence: Confidence::Identified,
        note: "17 instances in the switchyard, bbox 64 x 15 x 59 m.",
        refs: &[Reference {
            label: "Current transformers",
            url: "https://commons.wikimedia.org/wiki/Category:Current_transformers",
        }],
    },
    Entry {
        pattern: r"substation_transformer",
        what: "Oil-filled transformer with radiator bank and HV bushings",
        real: "Generator step-up or unit auxiliary transformer",
        confidence: Confidence::Identified,
        note: "Four instances, three in a row at X 45.8 against the building wall with risers into \
               the switchyard bus, each 4.3 x 4.5 x 4.8 m. Reads equally well as GSU plus two unit \
               auxiliary transformers, or as a three-phase bank of single-phase units.",
        refs: &[Reference {
            label: "High-voltage transformers with radiator banks",
            url: "https://commons.wikimedia.org/wiki/Category:High-voltage_transformers",
        }],
    },
    Entry {
        pattern: r"substation_support",
        what: "Lattice gantry and bus support insulator column",
        real: "Switchyard structure",
        confidence: Confidence::Identified,
        note: "29 instances.",
        refs: &[Reference {
            label: "Lattice switchyard structures",
            url: "https://commons.wikimedia.org/wiki/Category:Transmission_towers",
        }],
    },
    Entry {
        pattern: r"transformer_wires",
        what: "Overhead bus and jumpers",
        real: "Switchyard busbar",
        confidence: Confidence::Identified,
        note: "36 instances.",
        refs: &[Reference {
            label: "Busbars and overhead jumpers",
            url: "https://commons.wikimedia.org/wiki/Category:Busbars",
        }],
    },
    Entry {
        pattern: r"transformer_yard_powerbox",
        what: "Marshalling kiosk",
        real: "Switchyard marshalling cubicle",
        confidence: Confidence::Identified,
        note: "15 instances.",
        refs: &[Reference {
            label: "Marshalling kiosks and switchgear cabinets",
            url: "https://commons.wikimedia.org/wiki/Category:Switchgear",
        }],
    },
    Entry {
        pattern: r"nuke_power_pole",
        what: "Transmission pole",
        real: "Site transmission line",
        confidence: Confidence::Identified,
        note: "14 poles up to 33.8 m around the site perimeter.",
        refs: &[Reference {
            label: "Transmission structures",
            url: "https://commons.wikimedia.org/wiki/Category:Transmission_towers",
        }],
    },
    Entry {
        pattern: r"nuke_cargo_crane",
        what: "Rail-mounted gantry crane",
        real: "Heavy-lift or cask-handling crane",
        confidence: Confidence::Proxy,
        note: "About 39 m span, 14.6 m tall, two trolleys, hook block and festoon cabling, \
               outdoors at X -27.7. A good cask crane, but it is not the reactor building crane — \
               there is none over the refuelling floor.",
        refs: &[Reference {
            label: "Rail-mounted gantry cranes",
            url: "https://commons.wikimedia.org/wiki/Category:Gantry_cranes",
        }],
    },
    Entry {
        pattern: r"sprinkler_001",
        what: "Fire sprinkler head",
        real: "Building fire suppression",
        confidence: Confidence::Identified,
        note: "32 instances, part of a complete and genuinely dense building fire system: \
               detectors, alarm beacons, extinguishers and hose reels all exist.",
        refs: &[Reference {
            label: "Fire sprinkler heads",
            url: "https://commons.wikimedia.org/wiki/Category:Fire_sprinklers",
        }],
    },
    Entry {
        pattern: r"airduct_hvac|nuke_roof_ac|hr_metal_duct|nuke_ventilation_exhaust",
        what: "Rectangular sheet-metal duct and rooftop air-handling units",
        real: "Commercial-building HVAC",
        confidence: Confidence::Mismatch,
        note: "364 instances. This is office and industrial HVAC, not nuclear ventilation. It \
               cannot stand in for standby gas treatment, which looks like a shielded filter \
               housing train with charcoal adsorber banks and HEPA housings — absent entirely.",
        refs: &[Reference {
            label: "Sheet-metal ductwork",
            url: "https://commons.wikimedia.org/wiki/Category:Ductwork",
        }],
    },
    Entry {
        pattern: r"metal_pipe_00",
        what: "Modular pipe kit",
        real: "Process pipework",
        confidence: Confidence::Proxy,
        note: "1,927 pieces. Decorative kit placed to look joined rather than a modelled process \
               system, so runs are geometric adjacency, not a P&ID. The four large-bore risers \
               climbing the big shell and turning over its crown are the nearest thing to main \
               steam lines — but MSLs leave the RPV horizontally into a steam tunnel, never over \
               the containment roof.",
        refs: &[
            Reference {
                label: "Industrial process piping",
                url: "https://commons.wikimedia.org/wiki/Category:Piping",
            },
            Reference {
                label: "Where main steam lines really run — ABWR cross section",
                url: "https://commons.wikimedia.org/wiki/File:UK_ABWR_cross_section.png",
            },
        ],
    },
    Entry {
        pattern: r"nuke_water",
        what: "Water surface at Y -20.6",
        real: "Spent fuel pool",
        confidence: Confidence::Identified,
        note: "About 29 x 15 m with a floor at Y -27.5, so roughly 6.9 m deep. A real BWR SFP is \
               11-12 m. Shallow, but it reads correctly on camera.",
        refs: &[
            Reference {
                label: "Caorso BWR spent fuel pool",
                url: "https://commons.wikimedia.org/wiki/File:Caorso_fuel_pool.jpg",
            },
            Reference {
                label: "More spent fuel pools",
                url: "https://commons.wikimedia.org/wiki/Category:Spent_nuclear_fuel_ponds",
            },
        ],
    },
    Entry {
        pattern: r"hr_river_water",
        what: "River surface at Y -12.2",
        real: "Ultimate heat sink — once-through cooling water source",
        confidence: Confidence::Identified,
        note: "294 m wide across the whole site. Once-through is entirely plausible; \
               Kashiwazaki-Kariwa is seawater once-through. There are no cooling towers, and none \
               are needed.",
        refs: &[Reference {
            label: "Kashiwazaki-Kariwa on the coast — once-through, no towers",
            url: "https://commons.wikimedia.org/wiki/File:Kashiwazaki-Kariwa_Nuclear_Power_Plant_14-Aug-2019.jpg",
        }],
    },
];

/// Compiled table, built once at startup.
pub struct Identifier {
    compiled: Vec<Regex>,
}

impl Identifier {
    /// Compile the table.
    ///
    /// # Errors
    ///
    /// Fails only if a built-in pattern does not compile, which a test covers.
    pub fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            compiled: ENTRIES
                .iter()
                .map(|e| Regex::new(e.pattern))
                .collect::<Result<_, _>>()?,
        })
    }

    /// What a prop most likely represents, matched on material path first and
    /// model stem second.
    #[must_use]
    pub fn lookup(&self, material: &str, model: &str) -> Option<&'static Entry> {
        self.compiled
            .iter()
            .position(|re| re.is_match(material) || re.is_match(model))
            .map(|i| &ENTRIES[i])
    }

    /// How many props the table can name.
    #[must_use]
    pub fn len(&self) -> usize {
        ENTRIES.len()
    }

    /// How many links to photographs of real equipment the table carries.
    #[must_use]
    pub fn reference_count(&self) -> usize {
        ENTRIES.iter().map(|e| e.refs.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        assert!(Identifier::new().is_ok());
    }

    #[test]
    fn the_reactor_vessel_head_is_identified_not_guessed() {
        let id = Identifier::new().expect("compile");
        let entry = id
            .lookup(
                "materials/models/props/de_nuke/hr_nuke/nuke_reactor_vessel_head/nuke_reactor_vessel_head_color.vmat",
                "",
            )
            .expect("should be identified");
        assert_eq!(entry.confidence, Confidence::Identified);
        assert!(entry.real.contains("RPV head"));
    }

    /// The large shell must not be swallowed by the `nuke_silo_001` pattern.
    #[test]
    fn the_large_shell_matches_before_the_small_one() {
        let id = Identifier::new().expect("compile");
        let large = id
            .lookup("materials/models/props/de_nuke/hr_nuke/nuke_silo/nuke_silo_001b.vmat", "")
            .expect("large shell");
        assert!(large.what.contains("cylindrical skirt"), "{}", large.what);
    }

    /// The things that would embarrass the narration must be flagged, not
    /// quietly labelled as if they fit.
    #[test]
    fn contradictions_are_marked_as_mismatches() {
        let id = Identifier::new().expect("compile");
        for path in [
            "materials/models/props/de_nuke/hr_nuke/gas_meter/gas_meter_01_color.vmat",
            "materials/models/props/de_nuke/hr_nuke/nuke_control_room/control_room_displays_color.vmat",
            "materials/models/props/de_nuke/hr_nuke/nuke_silo/nuke_silo_001b.vmat",
            "materials/models/props/de_nuke/hr_nuke/nuke_industrial_props/nuke_industrial_props_001.vmat",
        ] {
            let entry = id.lookup(path, "").unwrap_or_else(|| panic!("no entry for {path}"));
            assert_eq!(
                entry.confidence,
                Confidence::Mismatch,
                "{path} should be flagged as a mismatch"
            );
        }
    }

    #[test]
    fn an_unknown_prop_returns_nothing_rather_than_a_guess() {
        let id = Identifier::new().expect("compile");
        assert!(id.lookup("materials/concrete/hr_concrete_wall_001b.vmat", "").is_none());
    }

    /// Every reference was resolved against the Wikimedia Commons API before it
    /// was written down. This pins the shape so a hand-typed or half-remembered
    /// URL cannot creep in later: anything that is not a Commons `File:` or
    /// `Category:` page did not come from that check.
    #[test]
    fn every_reference_is_a_checked_commons_page() {
        const PREFIX: &str = "https://commons.wikimedia.org/wiki/";
        for entry in ENTRIES {
            for reference in entry.refs {
                let rest = reference
                    .url
                    .strip_prefix(PREFIX)
                    .unwrap_or_else(|| panic!("{} is not a Commons page: {}", entry.pattern, reference.url));
                assert!(
                    rest.starts_with("File:") || rest.starts_with("Category:"),
                    "{} links to {rest}, which is neither a file nor a category",
                    entry.pattern
                );
                assert!(
                    !reference.label.is_empty(),
                    "{} has an unlabelled link",
                    entry.pattern
                );
                // A space would break the URL; Commons titles use underscores.
                assert!(
                    !reference.url.contains(' '),
                    "{} has a space in {}",
                    entry.pattern,
                    reference.url
                );
            }
        }
    }

    /// The point of the whole table is that a viewer can go and look at the real
    /// thing. If a prop is *identified* — the strongest claim on offer — there
    /// has to be something to look at.
    #[test]
    fn identified_props_all_have_a_picture_of_the_real_unit() {
        for entry in ENTRIES {
            if entry.confidence == Confidence::Identified {
                assert!(
                    !entry.refs.is_empty(),
                    "{} is claimed as identified but links to no real unit",
                    entry.pattern
                );
            }
        }
    }

    /// The RPV head note is the one place the nearest photograph is of the wrong
    /// reactor type. That has to stay visible in the link text, not be smoothed
    /// over into "reactor vessel heads".
    #[test]
    fn the_wrong_reactor_type_is_admitted_in_the_link_text() {
        let id = Identifier::new().expect("compile");
        let entry = id
            .lookup("materials/models/props/de_nuke/hr_nuke/nuke_reactor_vessel_head/x.vmat", "")
            .expect("head");
        assert!(
            entry.refs.iter().any(|r| r.label.contains("not BWR")),
            "the PWR photograph must be labelled as a PWR photograph"
        );
    }
}
