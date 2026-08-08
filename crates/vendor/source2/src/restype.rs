//! Mapping from compiled-resource file extensions to resource types.

/// The kind of resource a `*_c` file holds, derived from its extension.
///
/// Only the types that appear in CS2 content are enumerated; anything else is
/// [`ResourceType::Unknown`], which still carries the extension text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceType {
    /// `.vpk_c` style entry that is not actually a known resource.
    Unknown(String),
    /// `vmap_c` — compiled map.
    Map,
    /// `vwrld_c` — world.
    World,
    /// `vwnod_c` — world node.
    WorldNode,
    /// `vvis_c` — world visibility data.
    WorldVisibility,
    /// `vents_c` — entity lump.
    EntityLump,
    /// `vmdl_c` — model.
    Model,
    /// `vmesh_c` — mesh.
    Mesh,
    /// `vphys_c` — physics collision data.
    PhysicsCollisionMesh,
    /// `vmat_c` — material.
    Material,
    /// `vtex_c` — texture.
    Texture,
    /// `vpcf_c` — particle system.
    ParticleSystem,
    /// `vsnd_c` — sound.
    Sound,
    /// `vsndevts_c` — sound event script.
    SoundEventScript,
    /// `vsndstck_c` — sound stack script.
    SoundStackScript,
    /// `vanim_c` — animation.
    Animation,
    /// `vagrp_c` — animation group.
    AnimationGroup,
    /// `vseq_c` — sequence group.
    SequenceGroup,
    /// `vsurf_c` — surface properties.
    SurfaceProperties,
    /// `vpost_c` — post-processing settings.
    PostProcessing,
    /// `vrman_c` — resource manifest.
    ResourceManifest,
    /// `vdata_c` — generic keyvalues data.
    Data,
    /// `vsvg_c`, `vjs_c`, `vxml_c`, `vcss_c` — Panorama assets.
    Panorama,
    /// `vsmart_c` — smart prop.
    SmartProp,
    /// `vnmgraph_c`, `vnmclip_c`, `vnmskel_c` — NetworkedMotion animation.
    NmGraph,
    /// `vpulse_c` — Pulse graph.
    PulseGraph,
    /// `vcompmat_c` — composite material.
    CompositeMaterial,
    /// `vts_c` — texture set / sheet.
    TextureSheet,
}

impl ResourceType {
    /// Classify a file extension such as `vmdl_c` or `vmdl`.
    ///
    /// The trailing `_c` is optional; both compiled and source extensions map
    /// to the same type.
    #[must_use]
    pub fn from_extension(extension: &str) -> Self {
        // Lowercase before stripping so `.VWRLD_C` is handled too.
        let ext = extension.trim_start_matches('.').to_ascii_lowercase();
        let base = ext.strip_suffix("_c").unwrap_or(&ext).to_string();
        match base.as_str() {
            "vmap" => Self::Map,
            "vwrld" => Self::World,
            "vwnod" => Self::WorldNode,
            "vvis" => Self::WorldVisibility,
            "vents" => Self::EntityLump,
            "vmdl" => Self::Model,
            "vmesh" => Self::Mesh,
            "vphys" => Self::PhysicsCollisionMesh,
            "vmat" => Self::Material,
            "vtex" => Self::Texture,
            "vpcf" => Self::ParticleSystem,
            "vsnd" => Self::Sound,
            "vsndevts" => Self::SoundEventScript,
            "vsndstck" => Self::SoundStackScript,
            "vanim" => Self::Animation,
            "vagrp" => Self::AnimationGroup,
            "vseq" => Self::SequenceGroup,
            "vsurf" => Self::SurfaceProperties,
            "vpost" => Self::PostProcessing,
            "vrman" => Self::ResourceManifest,
            "vdata" => Self::Data,
            "vsvg" | "vjs" | "vxml" | "vcss" => Self::Panorama,
            "vsmart" => Self::SmartProp,
            "vnmgraph" | "vnmclip" | "vnmskel" => Self::NmGraph,
            "vpulse" => Self::PulseGraph,
            "vcompmat" => Self::CompositeMaterial,
            "vts" => Self::TextureSheet,
            _ => Self::Unknown(base),
        }
    }

    /// A short label for display.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Unknown(ext) if ext.is_empty() => "unknown".to_string(),
            Self::Unknown(ext) => format!("unknown({ext})"),
            other => format!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_compiled_and_source_extensions() {
        assert_eq!(ResourceType::from_extension("vmdl_c"), ResourceType::Model);
        assert_eq!(ResourceType::from_extension("vmdl"), ResourceType::Model);
        assert_eq!(
            ResourceType::from_extension(".VWRLD_C"),
            ResourceType::World
        );
        assert_eq!(
            ResourceType::from_extension("vphys_c"),
            ResourceType::PhysicsCollisionMesh
        );
    }

    #[test]
    fn unknown_extensions_keep_their_text() {
        let t = ResourceType::from_extension("vzzz_c");
        assert_eq!(t, ResourceType::Unknown("vzzz".to_string()));
        assert_eq!(t.label(), "unknown(vzzz)");
        assert_eq!(ResourceType::from_extension("").label(), "unknown");
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(
            ResourceType::from_extension("vents_c").label(),
            "EntityLump"
        );
    }
}
