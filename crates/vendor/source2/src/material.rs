//! Typed decoding of the material (`vmat_c`) resource.
//!
//! A compiled material is a KV3 `DATA` block naming a shader plus four
//! parallel parameter lists — ints, floats, vectors and texture references —
//! and a matching set of "attributes". Everything is kept as decoded, so a
//! caller can reach a parameter this crate has no opinion about; the
//! convenience accessors on top mirror the subset ValveResourceFormat's glTF
//! exporter treats as meaningful.

use crate::error::{Result, Source2Error};
use crate::kv3::KvValue;
use crate::resource::Resource;

/// How a material's alpha channel should be interpreted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaMode {
    /// Alpha is ignored.
    Opaque,
    /// Texels below the cutoff are discarded.
    Mask {
        /// Alpha value below which a texel is discarded.
        cutoff: f32,
    },
    /// Alpha blends the surface with what is behind it.
    Blend,
}

/// A decoded `vmat_c` resource.
#[derive(Debug, Clone, Default)]
pub struct Material {
    /// Material path as the compiler recorded it, e.g. `materials/foo.vmat`.
    pub name: String,
    /// Shader this material runs, e.g. `csgo_lightmappedgeneric.vfx`.
    pub shader: String,
    /// `m_intParams`, in file order. Names starting `F_` are shader features.
    pub int_params: Vec<(String, i64)>,
    /// `m_floatParams`.
    pub float_params: Vec<(String, f32)>,
    /// `m_vectorParams`.
    pub vector_params: Vec<(String, [f32; 4])>,
    /// `m_textureParams`: parameter name to referenced `*.vtex` path.
    pub texture_params: Vec<(String, String)>,
    /// `m_intAttributes`.
    pub int_attributes: Vec<(String, i64)>,
    /// `m_floatAttributes`.
    pub float_attributes: Vec<(String, f32)>,
    /// `m_vectorAttributes`.
    pub vector_attributes: Vec<(String, [f32; 4])>,
    /// `m_stringAttributes`.
    pub string_attributes: Vec<(String, String)>,
}

/// Texture parameter names that can carry the base colour, best first.
///
/// The blend shaders expose several colour layers; glTF has one base colour
/// slot, so the first layer wins.
pub const BASE_COLOR_PARAMS: &[&str] = &[
    "g_tColor",
    "g_tColor1",
    "g_tColorA",
    "g_tColor0",
    "g_tColorB",
    "g_tColor2",
    "g_tSkyTexture",
    "g_tDetail",
];

/// Texture parameter names that can carry a tangent-space normal map.
pub const NORMAL_PARAMS: &[&str] = &["g_tNormal", "g_tNormalA", "g_tNormal1", "g_tNormalB"];

impl Material {
    /// Decode a material resource.
    ///
    /// # Errors
    ///
    /// Returns [`Source2Error`] if the `DATA` block is absent or does not
    /// decode as KV3. A material whose `DATA` block uses the older NTRO struct
    /// encoding is reported as [`Source2Error::NotKv3`].
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource
            .data_kv3()
            .ok_or(Source2Error::NotKv3 { fourcc: "DATA" })??;
        Ok(Self::from_kv(&doc.root))
    }

    /// Decode a material from an already-parsed KV3 tree.
    #[must_use]
    pub fn from_kv(root: &KvValue) -> Self {
        Self {
            name: string_at(root, "m_materialName"),
            shader: string_at(root, "m_shaderName"),
            int_params: pairs(root, "m_intParams", "m_nValue", as_int),
            float_params: pairs(root, "m_floatParams", "m_flValue", as_float),
            vector_params: pairs(root, "m_vectorParams", "m_value", as_vec4),
            texture_params: pairs(root, "m_textureParams", "m_pValue", as_string),
            int_attributes: pairs(root, "m_intAttributes", "m_nValue", as_int),
            float_attributes: pairs(root, "m_floatAttributes", "m_flValue", as_float),
            vector_attributes: pairs(root, "m_vectorAttributes", "m_value", as_vec4),
            string_attributes: pairs(root, "m_stringAttributes", "m_value", as_string),
        }
    }

    /// Look up an int parameter.
    #[must_use]
    pub fn int_param(&self, name: &str) -> Option<i64> {
        self.int_params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
    }

    /// Look up a float parameter.
    #[must_use]
    pub fn float_param(&self, name: &str) -> Option<f32> {
        self.float_params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
    }

    /// Look up a vector parameter.
    #[must_use]
    pub fn vector_param(&self, name: &str) -> Option<[f32; 4]> {
        self.vector_params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
    }

    /// Look up a texture reference by parameter name.
    #[must_use]
    pub fn texture_param(&self, name: &str) -> Option<&str> {
        self.texture_params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The shader feature flags, i.e. the int params named `F_...`.
    #[must_use]
    pub fn features(&self) -> Vec<(&str, i64)> {
        self.int_params
            .iter()
            .filter(|(k, _)| k.starts_with("F_"))
            .map(|(k, v)| (k.as_str(), *v))
            .collect()
    }

    /// The first non-empty texture reference among `names`.
    #[must_use]
    pub fn first_texture(&self, names: &[&str]) -> Option<&str> {
        names
            .iter()
            .find_map(|n| self.texture_param(n).filter(|v| !v.is_empty()))
    }

    /// The texture to use as glTF `baseColorTexture`.
    #[must_use]
    pub fn base_color_texture(&self) -> Option<&str> {
        self.first_texture(BASE_COLOR_PARAMS)
    }

    /// The texture to use as glTF `normalTexture`.
    #[must_use]
    pub fn normal_texture(&self) -> Option<&str> {
        self.first_texture(NORMAL_PARAMS)
    }

    /// Whether the shader draws blended by nature rather than by flag.
    ///
    /// `csgo_effects.vfx` is the smoke, dust and steam shader: its geometry is
    /// a stack of cards whose shape comes from mask textures and vertex colour
    /// at draw time, and it is never opaque. `F_ADDITIVE_BLEND` says the same
    /// thing explicitly.
    #[must_use]
    pub fn is_effect_shader(&self) -> bool {
        self.shader.starts_with("csgo_effects")
            || self.int_param("F_ADDITIVE_BLEND").unwrap_or(0) > 0
    }

    /// How the alpha channel should be treated.
    ///
    /// `F_TRANSLUCENT` wins over `F_ALPHA_TEST`, and any `*_glass.vfx` shader
    /// is translucent whether or not it says so — both rules are Valve's. The
    /// effect shaders in [`Self::is_effect_shader`] are treated as translucent
    /// too, which Valve's own exporter does not do.
    #[must_use]
    pub fn alpha_mode(&self) -> AlphaMode {
        let translucent = self.int_param("F_TRANSLUCENT").unwrap_or(0) > 0
            || self.shader.ends_with("_glass.vfx")
            || self.is_effect_shader();
        if translucent {
            return AlphaMode::Blend;
        }
        if self.int_param("F_ALPHA_TEST").unwrap_or(0) > 0 {
            return AlphaMode::Mask {
                cutoff: self.float_param("g_flAlphaTestReference").unwrap_or(0.5),
            };
        }
        AlphaMode::Opaque
    }

    /// Whether back faces should be drawn.
    #[must_use]
    pub fn double_sided(&self) -> bool {
        self.int_param("F_RENDER_BACKFACES").unwrap_or(0) > 0
            || self.int_param("F_NO_CULLING").unwrap_or(0) > 0
    }

    /// Whether the shader ignores lighting.
    #[must_use]
    pub fn unlit(&self) -> bool {
        self.int_param("F_UNLIT").unwrap_or(0) > 0
    }

    /// Constant metalness, clamped to `0..=1`.
    #[must_use]
    pub fn metalness(&self) -> f32 {
        self.float_param("g_flMetalness")
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }

    /// Constant colour tint, defaulting to white.
    #[must_use]
    pub fn color_tint(&self) -> [f32; 4] {
        let mut tint = self.vector_param("g_vColorTint").unwrap_or([1.0; 4]);
        tint[3] = 1.0; // the tint never affects opacity
        for c in &mut tint {
            *c = c.clamp(0.0, 1.0);
        }
        tint
    }
}

fn string_at(root: &KvValue, key: &str) -> String {
    root.get(key)
        .and_then(KvValue::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Read a `[{ m_name, <value_key> }, ...]` array into name/value pairs.
fn pairs<T>(
    root: &KvValue,
    array_key: &str,
    value_key: &str,
    convert: fn(&KvValue) -> Option<T>,
) -> Vec<(String, T)> {
    root.get(array_key)
        .and_then(KvValue::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|entry| {
            let name = entry.get("m_name")?.as_str()?.to_string();
            Some((name, convert(entry.get(value_key)?)?))
        })
        .collect()
}

fn as_int(v: &KvValue) -> Option<i64> {
    v.as_i64()
}

#[allow(clippy::cast_possible_truncation)]
fn as_float(v: &KvValue) -> Option<f32> {
    Some(v.as_f64()? as f32)
}

fn as_string(v: &KvValue) -> Option<String> {
    Some(v.as_str()?.to_string())
}

/// A vector param is either a four-element array or an object with `m_x`-style
/// fields depending on how the KV3 was produced; accept both.
fn as_vec4(v: &KvValue) -> Option<[f32; 4]> {
    #[allow(clippy::cast_possible_truncation)]
    let f = |v: &KvValue| -> f32 { v.as_f64().unwrap_or(0.0) as f32 };
    if let Some(items) = v.as_array() {
        let mut out = [0.0f32; 4];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = items.get(i).map_or(0.0, f);
        }
        return Some(out);
    }
    if v.as_object().is_some() {
        return Some([
            v.get("0").or_else(|| v.get("x")).map_or(0.0, f),
            v.get("1").or_else(|| v.get("y")).map_or(0.0, f),
            v.get("2").or_else(|| v.get("z")).map_or(0.0, f),
            v.get("3").or_else(|| v.get("w")).map_or(0.0, f),
        ]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, key: &str, value: KvValue) -> KvValue {
        KvValue::Object(vec![
            ("m_name".into(), KvValue::String(name.into())),
            (key.into(), value),
        ])
    }

    fn sample() -> KvValue {
        KvValue::Object(vec![
            (
                "m_materialName".into(),
                KvValue::String("materials/de_dust2/wall.vmat".into()),
            ),
            (
                "m_shaderName".into(),
                KvValue::String("csgo_lightmappedgeneric.vfx".into()),
            ),
            (
                "m_intParams".into(),
                KvValue::Array(vec![
                    named("F_ALPHA_TEST", "m_nValue", KvValue::Int(1)),
                    named("F_RENDER_BACKFACES", "m_nValue", KvValue::Int(1)),
                    named("g_nScrollMode", "m_nValue", KvValue::Int(2)),
                ]),
            ),
            (
                "m_floatParams".into(),
                KvValue::Array(vec![
                    named("g_flAlphaTestReference", "m_flValue", KvValue::Double(0.4)),
                    named("g_flMetalness", "m_flValue", KvValue::Double(0.25)),
                ]),
            ),
            (
                "m_vectorParams".into(),
                KvValue::Array(vec![named(
                    "g_vColorTint",
                    "m_value",
                    KvValue::Array(vec![
                        KvValue::Double(0.5),
                        KvValue::Double(0.6),
                        KvValue::Double(0.7),
                        KvValue::Double(0.2),
                    ]),
                )]),
            ),
            (
                "m_textureParams".into(),
                KvValue::Array(vec![
                    named(
                        "g_tColor",
                        "m_pValue",
                        KvValue::String("materials/de_dust2/wall_color.vtex".into()),
                    ),
                    named(
                        "g_tNormal",
                        "m_pValue",
                        KvValue::String("materials/de_dust2/wall_normal.vtex".into()),
                    ),
                    named(
                        "g_tAmbientOcclusion",
                        "m_pValue",
                        KvValue::String(String::new()),
                    ),
                ]),
            ),
            (
                "m_stringAttributes".into(),
                KvValue::Array(vec![named(
                    "physicsSurfaceProperty",
                    "m_value",
                    KvValue::String("concrete".into()),
                )]),
            ),
        ])
    }

    #[test]
    fn decodes_every_parameter_list() {
        let m = Material::from_kv(&sample());
        assert_eq!(m.name, "materials/de_dust2/wall.vmat");
        assert_eq!(m.shader, "csgo_lightmappedgeneric.vfx");
        assert_eq!(m.int_params.len(), 3);
        assert_eq!(m.int_param("g_nScrollMode"), Some(2));
        assert_eq!(m.float_param("g_flMetalness"), Some(0.25));
        assert_eq!(m.vector_param("g_vColorTint").unwrap()[1], 0.6);
        assert_eq!(m.texture_params.len(), 3);
        assert_eq!(
            m.string_attributes,
            vec![("physicsSurfaceProperty".to_string(), "concrete".to_string())]
        );
    }

    #[test]
    fn picks_colour_and_normal_textures_and_skips_empties() {
        let m = Material::from_kv(&sample());
        assert_eq!(
            m.base_color_texture(),
            Some("materials/de_dust2/wall_color.vtex")
        );
        assert_eq!(
            m.normal_texture(),
            Some("materials/de_dust2/wall_normal.vtex")
        );
        // An empty reference must not be offered as a texture.
        assert_eq!(m.first_texture(&["g_tAmbientOcclusion"]), None);
    }

    #[test]
    fn maps_alpha_and_culling_flags() {
        let m = Material::from_kv(&sample());
        assert_eq!(m.alpha_mode(), AlphaMode::Mask { cutoff: 0.4 });
        assert!(m.double_sided());
        assert!(!m.unlit());
        assert_eq!(m.features().len(), 2);
        // The tint is clamped and its alpha forced opaque.
        assert_eq!(m.color_tint(), [0.5, 0.6, 0.7, 1.0]);
    }

    #[test]
    fn translucency_wins_over_alpha_test() {
        let mut root = sample();
        if let KvValue::Object(fields) = &mut root {
            for (k, v) in fields.iter_mut() {
                if k == "m_intParams" {
                    if let KvValue::Array(items) = v {
                        items.push(named("F_TRANSLUCENT", "m_nValue", KvValue::Int(1)));
                    }
                }
            }
        }
        assert_eq!(Material::from_kv(&root).alpha_mode(), AlphaMode::Blend);
    }

    #[test]
    fn glass_shaders_are_translucent_without_a_flag() {
        let root = KvValue::Object(vec![(
            "m_shaderName".into(),
            KvValue::String("csgo_glass.vfx".into()),
        )]);
        assert_eq!(Material::from_kv(&root).alpha_mode(), AlphaMode::Blend);
    }

    #[test]
    fn missing_fields_default_rather_than_fail() {
        let m = Material::from_kv(&KvValue::Null);
        assert!(m.name.is_empty());
        assert_eq!(m.alpha_mode(), AlphaMode::Opaque);
        assert_eq!(m.metalness(), 0.0);
        assert_eq!(m.color_tint(), [1.0, 1.0, 1.0, 1.0]);
        assert!(m.base_color_texture().is_none());
    }

    #[test]
    fn vector_params_accept_object_form() {
        let root = KvValue::Object(vec![(
            "m_vectorParams".into(),
            KvValue::Array(vec![named(
                "g_vColorTint",
                "m_value",
                KvValue::Object(vec![
                    ("0".into(), KvValue::Double(0.1)),
                    ("1".into(), KvValue::Double(0.2)),
                    ("2".into(), KvValue::Double(0.3)),
                    ("3".into(), KvValue::Double(0.4)),
                ]),
            )]),
        )]);
        let m = Material::from_kv(&root);
        assert_eq!(m.vector_param("g_vColorTint"), Some([0.1, 0.2, 0.3, 0.4]));
    }
}
