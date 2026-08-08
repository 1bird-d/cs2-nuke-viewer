//! What each instance *is*, and how it should be drawn.
//!
//! Classification runs once at load: every instance is matched against an
//! ordered list of patterns and assigned exactly one category. The match is
//! against the Valve material path first, because that names the source art kit
//! exactly — `.../hr_nuke/metal_pipe_001/metal_pipe_001.vmat` is unambiguous in
//! a way that a model stem baked out of an aggregate is not.
//!
//! Categories are ordered most specific first, and the first hit wins. That is
//! why `nuke_reactor_vessel_head` lands in `vessel` rather than being caught by
//! something broader further down.
//!
//! Everything about *display* — the colour and whether a category is solid,
//! ghosted or hidden — is mutable at runtime. Nothing here requires a re-bake or
//! even a reload; the viewer reclassifies and re-uploads in single-digit
//! milliseconds, so a toggle is instant.

use anyhow::Result;
use nkp_format::Scene;
use regex::Regex;

/// How a category is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Opaque, lit, in the category colour.
    Solid,
    /// A fresnel shell: edges only, additive, no depth write. Keeps the shape
    /// of the building readable without hiding the plant inside it.
    Ghost,
    /// Not drawn at all.
    Hidden,
}

impl Mode {
    /// Cycle order for a click or a key press.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Solid => Self::Ghost,
            Self::Ghost => Self::Hidden,
            Self::Hidden => Self::Solid,
        }
    }

    /// Single-letter tag for the panel and the console.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Solid => "SOLID",
            Self::Ghost => "GHOST",
            Self::Hidden => "HIDDEN",
        }
    }
}

/// One classified group of geometry.
#[derive(Debug, Clone)]
pub struct Category {
    /// Short lowercase id, also the `--only` shorthand.
    pub id: &'static str,
    /// Name shown in the panel.
    pub label: &'static str,
    /// Linear RGB the category draws in.
    pub colour: [f32; 3],
    /// How it is drawn right now.
    pub mode: Mode,
    /// How many instances landed here.
    pub instances: u32,
    /// How many triangles those instances draw.
    pub triangles: u64,
}

/// The pattern and defaults for one category.
struct Def {
    id: &'static str,
    label: &'static str,
    pattern: &'static str,
    colour: [f32; 3],
    mode: Mode,
}

/// Ordered most specific first; first match wins.
///
/// Colours are chosen to stay distinct against the dark ground and against each
/// other, with the process categories warm and the supporting ones cool, so a
/// glance separates "plant" from "everything that holds the plant up".
const DEFS: &[Def] = &[
    Def {
        id: "pipe",
        label: "Pipework",
        pattern: r"metal_pipe_00|gas_meter_pipes",
        colour: [1.00, 0.34, 0.10],
        mode: Mode::Solid,
    },
    Def {
        id: "duct",
        label: "HVAC ducting",
        pattern: r"airduct_hvac|nuke_ventilation_exhaust|nuke_vent_slats|nuke_vent_bombsite|hr_metal_duct",
        colour: [0.30, 0.58, 1.00],
        mode: Mode::Solid,
    },
    Def {
        id: "vessel",
        label: "Vessels & tanks",
        pattern: r"nuke_reactor_vessel_head|nuke_spent_fuel_racks|nuke_silo|medium_silo",
        colour: [0.20, 0.90, 0.55],
        mode: Mode::Solid,
    },
    // `nuke_machinery_*` are pressure vessels, a skid and an electric motor —
    // not instruments. Lumping them in put 78% of "instrumentation" by triangle
    // count on four drums, which made the category useless for showing where
    // the plant is actually instrumented.
    Def {
        id: "machinery",
        label: "Machinery",
        pattern: r"nuke_machinery",
        colour: [0.25, 0.85, 0.92],
        mode: Mode::Solid,
    },
    // `gas_meter` is deliberately absent: it is a domestic diaphragm gas meter
    // with a cubic-metre odometer on its face, not plant instrumentation. Its
    // pipework still classifies as pipe through `gas_meter_pipes`.
    Def {
        id: "instrument",
        label: "Instrumentation",
        pattern: r"control_room_displays|nuke_electric_panel|nuke_office_desk_monitor",
        colour: [0.78, 0.45, 1.00],
        mode: Mode::Solid,
    },
    Def {
        id: "electrical",
        label: "Electrical",
        pattern: r"current_transformer|nuke_circuit_breaker|substation_support|transformer_wires|nuke_power_pole|power_outlet",
        colour: [1.00, 0.82, 0.20],
        mode: Mode::Solid,
    },
    Def {
        id: "cabling",
        label: "Cable runs",
        pattern: r"wires_00",
        colour: [0.85, 0.60, 0.30],
        mode: Mode::Hidden,
    },
    Def {
        id: "access",
        label: "Access steel",
        pattern: r"metal_ladder|metal_railing|catwalk_support|web_joist|hazard_stripe",
        colour: [0.55, 0.62, 0.72],
        mode: Mode::Ghost,
    },
    Def {
        id: "lighting",
        label: "Lighting",
        pattern: r"nuke_light_fixture|nuke_fluorescent|nuke_twin_spot|nuke_bell_light",
        colour: [0.95, 0.90, 0.70],
        mode: Mode::Hidden,
    },
    Def {
        id: "structure",
        label: "Building fabric",
        pattern: r"^materials/(concrete|metal|ground|ceiling|plaster|tile|brick|wood|glass|dev)/|hr_concrete|hr_metal_corrugated|metal_door|window_00|curbs_001|hr_plaster|roof_",
        colour: [0.50, 0.56, 0.66],
        mode: Mode::Ghost,
    },
    Def {
        id: "clutter",
        label: "Clutter & props",
        pattern: r"chainlink_fence|foliage|beech|nuke_office_|nuke_cars|nuke_clothes|nuke_hard_hat|nuke_recycle_bin|nuke_chair|nuke_locker|nuke_sink|nuke_overall|metal_crate|swat_van|forklift|fire_extinguisher|signs_00|hr_area_signs|nuke_bombsite|vending",
        colour: [0.40, 0.42, 0.48],
        mode: Mode::Hidden,
    },
];

/// The last category: anything no pattern claimed.
const OTHER: Def = Def {
    id: "other",
    label: "Unclassified",
    pattern: "",
    colour: [0.45, 0.48, 0.55],
    mode: Mode::Ghost,
};

/// Categories plus the per-instance assignment.
pub struct Classification {
    /// Display state, in [`DEFS`] order with `other` last.
    pub categories: Vec<Category>,
    /// Category index for each instance, parallel to `scene.instances()`.
    pub of_instance: Vec<u8>,
}

impl Classification {
    /// Classify every instance in the scene.
    ///
    /// # Errors
    ///
    /// Fails only if one of the built-in patterns does not compile, which is a
    /// bug in [`DEFS`] and is covered by a test.
    pub fn new(scene: &Scene) -> Result<Self> {
        let compiled: Vec<Regex> = DEFS
            .iter()
            .map(|d| Regex::new(d.pattern))
            .collect::<Result<_, regex::Error>>()?;

        let mut categories: Vec<Category> = DEFS
            .iter()
            .chain(std::iter::once(&OTHER))
            .map(|d| Category {
                id: d.id,
                label: d.label,
                colour: d.colour,
                mode: d.mode,
                instances: 0,
                triangles: 0,
            })
            .collect();

        let other_index = compiled.len();
        let materials = scene.materials();
        let mut of_instance = Vec::with_capacity(scene.instances().len());

        for instance in scene.instances() {
            let material = materials
                .get(instance.material as usize)
                .map_or("", |m| scene.material_name(m));
            let name = scene.instance_name(instance);
            let index = compiled
                .iter()
                .position(|re| re.is_match(material) || re.is_match(name))
                .unwrap_or(other_index);

            let category = &mut categories[index];
            category.instances += 1;
            category.triangles += u64::from(instance.index_count) / 3;
            #[allow(clippy::cast_possible_truncation)]
            of_instance.push(index as u8);
        }

        Ok(Self {
            categories,
            of_instance,
        })
    }

    /// The category an instance belongs to.
    #[must_use]
    pub fn category_of(&self, instance: usize) -> &Category {
        let index = self.of_instance.get(instance).copied().unwrap_or(0) as usize;
        &self.categories[index.min(self.categories.len() - 1)]
    }

    /// Index of a category by id.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.categories.iter().position(|c| c.id == id)
    }

    /// Set every category to one mode.
    pub fn set_all(&mut self, mode: Mode) {
        for category in &mut self.categories {
            category.mode = mode;
        }
    }

    /// Show the process plant solid and ghost the rest — the view the whole
    /// project exists to produce, on one key.
    pub fn preset_plant(&mut self) {
        const SOLID: &[&str] = &["pipe", "duct", "vessel", "machinery", "instrument"];
        const GHOST: &[&str] = &["structure", "access", "other"];
        for category in &mut self.categories {
            category.mode = if SOLID.contains(&category.id) {
                Mode::Solid
            } else if GHOST.contains(&category.id) {
                Mode::Ghost
            } else {
                Mode::Hidden
            };
        }
    }

    /// Pipework and instrumentation alone, nothing else drawn at all.
    pub fn preset_pipes_only(&mut self) {
        for category in &mut self.categories {
            category.mode = match category.id {
                "pipe" | "instrument" => Mode::Solid,
                _ => Mode::Hidden,
            };
        }
    }

    /// Everything solid, as the map really looks.
    pub fn preset_everything(&mut self) {
        self.set_all(Mode::Solid);
    }

    /// World AABB of everything not hidden, in metres.
    ///
    /// Framing the whole map when only the pipework is drawn leaves it a smudge
    /// in the middle of an empty frame, so the camera frames what is visible.
    /// Falls back to the scene bounds when nothing is drawn.
    #[must_use]
    pub fn visible_bounds(&self, scene: &Scene) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut any = false;
        for (id, instance) in scene.instances().iter().enumerate() {
            if self.category_of(id).mode == Mode::Hidden {
                continue;
            }
            any = true;
            for axis in 0..3 {
                min[axis] = min[axis].min(instance.aabb_min[axis]);
                max[axis] = max[axis].max(instance.aabb_max[axis]);
            }
        }
        if any {
            (min, max)
        } else {
            let header = scene.header();
            (header.scene_min, header.scene_max)
        }
    }

    /// Instances and triangles that will actually be drawn.
    #[must_use]
    pub fn drawn(&self) -> (u32, u64) {
        self.categories
            .iter()
            .filter(|c| c.mode != Mode::Hidden)
            .fold((0, 0), |(i, t), c| (i + c.instances, t + c.triangles))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        for def in DEFS {
            assert!(Regex::new(def.pattern).is_ok(), "{} does not compile", def.id);
        }
    }

    #[test]
    fn category_ids_are_unique() {
        let mut ids: Vec<&str> = DEFS.iter().map(|d| d.id).chain([OTHER.id]).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate category id");
    }

    /// Order matters: the reactor vessel head must not be swallowed by a
    /// broader pattern sitting above it.
    #[test]
    fn the_first_matching_category_wins_in_definition_order() {
        let compiled: Vec<Regex> = DEFS.iter().map(|d| Regex::new(d.pattern).unwrap()).collect();
        let classify = |path: &str| -> &str {
            compiled
                .iter()
                .position(|re| re.is_match(path))
                .map_or(OTHER.id, |i| DEFS[i].id)
        };

        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/metal_pipe_001/metal_pipe_001.vmat"),
            "pipe"
        );
        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/nuke_reactor_vessel_head/nuke_reactor_vessel_head_color.vmat"),
            "vessel"
        );
        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/airduct_hvac_001/airduct_hvac_001.vmat"),
            "duct"
        );
        // The ABWR review found both of these mislabelled as instrumentation:
        // the machinery props are pressure vessels and a motor, and the gas
        // meter is a domestic one. Pin them so they cannot drift back.
        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/nuke_machinery/nuke_machinery_05_color.vmat"),
            "machinery"
        );
        assert_ne!(
            classify("materials/models/props/de_nuke/hr_nuke/gas_meter/gas_meter_color.vmat"),
            "instrument",
            "a domestic gas meter is not plant instrumentation"
        );
        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/gas_meter/gas_meter_pipes_color.vmat"),
            "pipe"
        );
        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/wires_001/wires_001.vmat"),
            "cabling"
        );
        assert_eq!(classify("materials/concrete/hr_concrete_wall_001b.vmat"), "structure");
        assert_eq!(
            classify("materials/models/props/de_nuke/hr_nuke/chainlink_fence_001.vmat"),
            "clutter"
        );
        assert_eq!(classify("materials/models/props/de_nuke/hr_nuke/mystery_thing.vmat"), "other");
    }

    #[test]
    fn modes_cycle_back_to_where_they_started() {
        let mut mode = Mode::Solid;
        for _ in 0..3 {
            mode = mode.next();
        }
        assert_eq!(mode, Mode::Solid);
    }
}
