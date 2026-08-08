//! Typed decoding of the texture (`vtex_c`) resource.
//!
//! Unlike most modern Source 2 resources a `vtex_c` `DATA` block is *not* KV3:
//! it is a fixed struct followed by a table of optional "extra data" entries.
//! The pixel data lives immediately after the block, smallest mip first.
//!
//! Layout, following ValveResourceFormat's `Texture.cs`:
//!
//! ```text
//! u16   version           (always 1)
//! u16   flags             VTexFlags
//! f32x4 reflectivity
//! u16   width             power-of-two storage size
//! u16   height
//! u16   depth
//! u8    format            VTexFormat
//! u8    mip_count
//! u32   picmip0_res
//! u32   extra_data_offset relative to its own position
//! u32   extra_data_count
//! ```
//!
//! Each extra-data entry is `u32 kind, u32 offset, u32 size`, the offset again
//! being relative to its own position. Two kinds matter here:
//!
//! * `METADATA` carries the true (non power of two) dimensions;
//! * `COMPRESSED_MIP_SIZE` carries a per-mip byte count and a flag saying
//!   whether those mips are LZ4 block-compressed. CS2 uses this for almost
//!   everything.
//!
//! # Colour codecs
//!
//! Some textures are stored in a swizzled or alternative colour space and the
//! algorithm used is recorded in the resource edit info (`RED2`) rather than in
//! the texture header. [`TextureCodec`] recovers it, and [`Texture::decode_mip`]
//! applies it, so a decoded image is always plain RGBA8.

use crate::error::{Result, Source2Error};
use crate::kv3::KvValue;
use crate::reader::Reader;
use crate::resource::{FourCc, Resource};

/// Ceiling on a single decoded mip, to keep a corrupt header from asking for a
/// huge allocation (1 GiB).
const MAX_MIP_BYTES: u64 = 1024 * 1024 * 1024;

/// Pixel format of the stored data. Values match Valve's `VTexFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TextureFormat {
    Unknown,
    /// BC1.
    Dxt1,
    /// BC3.
    Dxt5,
    I8,
    Rgba8888,
    R16,
    Rg1616,
    Rgba16161616,
    R16F,
    Rg1616F,
    Rgba16161616F,
    R32F,
    Rg3232F,
    Rgb323232F,
    Rgba32323232F,
    JpegRgba8888,
    PngRgba8888,
    JpegDxt5,
    PngDxt5,
    Bc6H,
    Bc7,
    /// BC5.
    Ati2n,
    Ia88,
    Etc2,
    Etc2Eac,
    R11Eac,
    Rg11Eac,
    /// BC4.
    Ati1n,
    Bgra8888,
    WebpRgba8888,
    WebpDxt5,
    /// A format byte this decoder does not know.
    Other(u8),
}

impl TextureFormat {
    /// Map the format byte from the header.
    #[must_use]
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Unknown,
            1 => Self::Dxt1,
            2 => Self::Dxt5,
            3 => Self::I8,
            4 => Self::Rgba8888,
            5 => Self::R16,
            6 => Self::Rg1616,
            7 => Self::Rgba16161616,
            8 => Self::R16F,
            9 => Self::Rg1616F,
            10 => Self::Rgba16161616F,
            11 => Self::R32F,
            12 => Self::Rg3232F,
            13 => Self::Rgb323232F,
            14 => Self::Rgba32323232F,
            15 => Self::JpegRgba8888,
            16 => Self::PngRgba8888,
            17 => Self::JpegDxt5,
            18 => Self::PngDxt5,
            19 => Self::Bc6H,
            20 => Self::Bc7,
            21 => Self::Ati2n,
            22 => Self::Ia88,
            23 => Self::Etc2,
            24 => Self::Etc2Eac,
            25 => Self::R11Eac,
            26 => Self::Rg11Eac,
            27 => Self::Ati1n,
            28 => Self::Bgra8888,
            29 => Self::WebpRgba8888,
            30 => Self::WebpDxt5,
            other => Self::Other(other),
        }
    }

    /// Short name, as Valve spells it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Dxt1 => "DXT1",
            Self::Dxt5 => "DXT5",
            Self::I8 => "I8",
            Self::Rgba8888 => "RGBA8888",
            Self::R16 => "R16",
            Self::Rg1616 => "RG1616",
            Self::Rgba16161616 => "RGBA16161616",
            Self::R16F => "R16F",
            Self::Rg1616F => "RG1616F",
            Self::Rgba16161616F => "RGBA16161616F",
            Self::R32F => "R32F",
            Self::Rg3232F => "RG3232F",
            Self::Rgb323232F => "RGB323232F",
            Self::Rgba32323232F => "RGBA32323232F",
            Self::JpegRgba8888 => "JPEG_RGBA8888",
            Self::PngRgba8888 => "PNG_RGBA8888",
            Self::JpegDxt5 => "JPEG_DXT5",
            Self::PngDxt5 => "PNG_DXT5",
            Self::Bc6H => "BC6H",
            Self::Bc7 => "BC7",
            Self::Ati2n => "ATI2N",
            Self::Ia88 => "IA88",
            Self::Etc2 => "ETC2",
            Self::Etc2Eac => "ETC2_EAC",
            Self::R11Eac => "R11_EAC",
            Self::Rg11Eac => "RG11_EAC",
            Self::Ati1n => "ATI1N",
            Self::Bgra8888 => "BGRA8888",
            Self::WebpRgba8888 => "WEBP_RGBA8888",
            Self::WebpDxt5 => "WEBP_DXT5",
            Self::Other(_) => "OTHER",
        }
    }

    /// Bytes per block (compressed formats) or per pixel (plain formats).
    ///
    /// Note that `ATI2N` reports 1 here, not 16. That looks wrong in isolation
    /// but is what Valve's own tooling uses: `ATI2N` is excluded from the
    /// block-size path below, so its mip size comes out as `width * height`,
    /// which happens to be exactly the BC5 block-compressed size.
    #[must_use]
    pub fn block_size(self) -> usize {
        match self {
            Self::Dxt1 | Self::Ati1n | Self::Etc2 => 8,
            Self::Dxt5 | Self::Bc6H | Self::Bc7 | Self::Etc2Eac | Self::Rgba32323232F => 16,
            Self::Rgba8888 | Self::Rg1616 | Self::Rg1616F | Self::R32F | Self::Bgra8888 => 4,
            Self::R16 | Self::R16F | Self::Ia88 => 2,
            Self::Rgba16161616 | Self::Rgba16161616F | Self::Rg3232F => 8,
            Self::Rgb323232F => 12,
            _ => 1,
        }
    }

    /// Whether mip sizes use the 4x4 block padding rule.
    ///
    /// `ATI2N` is deliberately absent, matching Valve. See [`Self::block_size`].
    #[must_use]
    pub fn is_block_padded(self) -> bool {
        matches!(
            self,
            Self::Dxt1
                | Self::Dxt5
                | Self::Bc6H
                | Self::Bc7
                | Self::Etc2
                | Self::Etc2Eac
                | Self::Ati1n
        )
    }

    /// Bytes per 4x4 block for the BCn formats this decoder understands.
    fn bcn_block_bytes(self) -> Option<usize> {
        Some(match self {
            Self::Dxt1 | Self::Ati1n => 8,
            Self::Dxt5 | Self::Bc7 | Self::Ati2n | Self::Bc6H => 16,
            _ => return None,
        })
    }
}

/// Header flag bits. Values match Valve's `VTexFlags`.
pub mod flags {
    /// Clamp the S axis rather than repeating.
    pub const SUGGEST_CLAMP_S: u16 = 1 << 0;
    /// Clamp the T axis.
    pub const SUGGEST_CLAMP_T: u16 = 1 << 1;
    /// Clamp the U axis.
    pub const SUGGEST_CLAMP_U: u16 = 1 << 2;
    /// The texture has no mip chain.
    pub const NO_LOD: u16 = 1 << 3;
    /// Six faces per slice.
    pub const CUBE_TEXTURE: u16 = 1 << 4;
    /// The depth axis is a real volume, and mips shrink it too.
    pub const VOLUME_TEXTURE: u16 = 1 << 5;
    /// The depth axis is an array of independent images.
    pub const TEXTURE_ARRAY: u16 = 1 << 6;
    /// Panorama: dilate the colour into transparent texels.
    pub const PANORAMA_DILATE_COLOR: u16 = 1 << 7;
    /// Panorama: stored as `YCoCg` in DXT5.
    pub const PANORAMA_CONVERT_TO_YCOCG_DXT5: u16 = 1 << 8;
    /// The API texture is created with a linear (not sRGB) view.
    pub const CREATE_LINEAR_API_TEXTURE: u16 = 1 << 9;
}

/// Kind tag of an extra-data entry. Values match Valve's `VTexExtraData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ExtraDataKind {
    Unknown,
    FallbackBits,
    Sheet,
    Metadata,
    CompressedMipSize,
    CubemapRadianceSh,
    Other(u32),
}

impl ExtraDataKind {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::FallbackBits,
            2 => Self::Sheet,
            3 => Self::Metadata,
            4 => Self::CompressedMipSize,
            5 => Self::CubemapRadianceSh,
            other => Self::Other(other),
        }
    }
}

/// One raw extra-data entry, kept so callers can inspect kinds this decoder
/// does not interpret.
#[derive(Debug, Clone)]
pub struct ExtraData {
    /// What the entry holds.
    pub kind: ExtraDataKind,
    /// The entry's bytes.
    pub bytes: Vec<u8>,
}

/// Post-decode colour transforms, recovered from the resource edit info.
///
/// Valve records the compile-time image processor in `RED2`'s special
/// dependency list rather than in the texture header, so a decoder that only
/// looks at [`TextureFormat`] produces wrong colours for normal maps and for
/// `YCoCg` skyboxes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextureCodec {
    /// Stored as `YCoCg` in the RGB channels with luma in alpha.
    pub ycocg: bool,
    /// Red and alpha are swapped (DXT5nm normal maps).
    pub dxt5nm: bool,
    /// RG hold a hemi-octahedral normal and B holds roughness.
    pub hemi_oct: bool,
    /// RG hold a normal whose Z must be reconstructed.
    pub normalize_normals: bool,
    /// `YCoCg` data that is in sRGB gamma space (cube maps).
    pub srgb: bool,
}

impl TextureCodec {
    /// Whether any transform is needed.
    #[must_use]
    pub fn is_identity(self) -> bool {
        !(self.ycocg || self.dxt5nm || self.hemi_oct || self.normalize_normals)
    }

    /// Read the codec out of a resource's `RED2` edit-info block.
    ///
    /// Returns the identity codec when there is no `RED2`, it does not decode,
    /// or it names no texture compiler dependency.
    #[must_use]
    pub fn from_resource(resource: &Resource<'_>, format: TextureFormat, cube: bool) -> Self {
        let mut codec = Self::default();

        if let Some(Ok(doc)) = resource.kv3_block(FourCc::RED2) {
            let deps = doc
                .get("m_SpecialDependencies")
                .and_then(KvValue::as_array)
                .unwrap_or(&[]);
            for dep in deps {
                if dep.get("m_CompilerIdentifier").and_then(KvValue::as_str)
                    != Some("CompileTexture")
                {
                    continue;
                }
                match dep.get("m_String").and_then(KvValue::as_str) {
                    Some("Texture Compiler Version Image YCoCg Conversion") => codec.ycocg = true,
                    Some("Texture Compiler Version Image NormalizeNormals") => {
                        codec.normalize_normals = true;
                    }
                    Some(
                        "Texture Compiler Version Mip HemiOctIsoRoughness_RG_B"
                        | "Texture Compiler Version Mip HemiOctAnisoRoughness",
                    ) => codec.hemi_oct = true,
                    _ => {}
                }
            }
        }

        if codec.ycocg && cube {
            codec.srgb = true;
        }
        if format == TextureFormat::Dxt5 && codec.normalize_normals {
            codec.dxt5nm = true;
        } else if format == TextureFormat::Bc7 && codec.hemi_oct && codec.normalize_normals {
            codec.normalize_normals = false;
        }

        codec
    }
}

/// A decoded image: tightly packed 8-bit RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, row-major, no padding.
    pub rgba: Vec<u8>,
}

impl Image {
    /// Allocate a transparent-black image.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    /// Whether every texel is fully opaque.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.rgba.chunks_exact(4).all(|p| p[3] == 255)
    }

    /// Halve the image with a 2x2 box filter, repeatedly, until neither axis
    /// exceeds `max_size`. A `max_size` of zero is ignored.
    pub fn downscale_to(&mut self, max_size: u32) {
        if max_size == 0 {
            return;
        }
        while (self.width > max_size || self.height > max_size) && self.width > 1 && self.height > 1
        {
            *self = self.halved();
        }
    }

    /// One 2x2 box-filter reduction. Odd sizes round down but never to zero.
    #[must_use]
    pub fn halved(&self) -> Self {
        let w = (self.width / 2).max(1);
        let h = (self.height / 2).max(1);
        let mut out = Self::new(w, h);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let mut sum = [0u32; 4];
                let mut n = 0u32;
                for dy in 0..2usize {
                    let sy = y * 2 + dy;
                    if sy >= self.height as usize {
                        continue;
                    }
                    for dx in 0..2usize {
                        let sx = x * 2 + dx;
                        if sx >= self.width as usize {
                            continue;
                        }
                        let i = (sy * self.width as usize + sx) * 4;
                        for (channel, byte) in sum.iter_mut().zip(&self.rgba[i..i + 4]) {
                            *channel += u32::from(*byte);
                        }
                        n += 1;
                    }
                }
                let o = (y * w as usize + x) * 4;
                let n = n.max(1);
                for (slot, channel) in out.rgba[o..o + 4].iter_mut().zip(sum.iter()) {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        *slot = (channel / n) as u8;
                    }
                }
            }
        }
        out
    }
}

/// A parsed `vtex_c` texture. Borrows the file bytes.
#[derive(Debug, Clone)]
pub struct Texture<'a> {
    data: &'a [u8],
    /// Offset of the first byte of pixel data (just past the `DATA` block).
    data_offset: usize,
    /// Header version; always 1.
    pub version: u16,
    /// Raw flag bits; see [`flags`].
    pub flags: u16,
    /// Average colour, as recorded by the compiler.
    pub reflectivity: [f32; 4],
    /// Storage width, always a power of two.
    pub width: u16,
    /// Storage height.
    pub height: u16,
    /// Slice count: array length, volume depth, or 1.
    pub depth: u16,
    /// Pixel format.
    pub format: TextureFormat,
    /// Number of mip levels stored, smallest first in the file.
    pub mip_count: u8,
    /// Resolution the compiler considers "picmip 0".
    pub picmip0_res: u32,
    /// True width when the source image was not a power of two, else 0.
    pub non_pow2_width: u16,
    /// True height when the source image was not a power of two, else 0.
    pub non_pow2_height: u16,
    /// Every extra-data entry, in file order.
    pub extra_data: Vec<ExtraData>,
    /// Per-mip stored byte counts, smallest mip first. Present when the
    /// `COMPRESSED_MIP_SIZE` entry is.
    pub compressed_mip_sizes: Option<Vec<u32>>,
    /// Whether those per-mip runs are actually LZ4 compressed.
    pub mips_are_compressed: bool,
    /// Colour transform to apply after decoding.
    pub codec: TextureCodec,
}

impl<'a> Texture<'a> {
    /// Parse the `DATA` block of a `vtex_c` resource.
    ///
    /// # Errors
    ///
    /// Returns [`Source2Error`] if there is no `DATA` block, the version is not
    /// 1, or any offset points outside the file. Never panics.
    pub fn from_resource(resource: &Resource<'a>) -> Result<Self> {
        let block = resource
            .block(FourCc::DATA)
            .ok_or(Source2Error::MissingBlock { fourcc: "DATA" })?;
        let data = resource.data();
        let mut r = Reader::new(data);
        r.seek(block.offset, "vtex header")?;

        let version = r.u16("vtex version")?;
        if version != 1 {
            return Err(Source2Error::UnsupportedTextureVersion { version });
        }
        let tex_flags = r.u16("vtex flags")?;
        let mut reflectivity = [0.0f32; 4];
        for slot in &mut reflectivity {
            *slot = f32::from_le_bytes(r.array::<4>("vtex reflectivity")?);
        }
        let width = r.u16("vtex width")?;
        let height = r.u16("vtex height")?;
        let depth = r.u16("vtex depth")?;
        let format = TextureFormat::from_byte(r.array::<1>("vtex format")?[0]);
        let mip_count = r.array::<1>("vtex mip count")?[0];
        let picmip0_res = r.u32("vtex picmip0 res")?;
        let extra_offset = r.u32("vtex extra data offset")?;
        let extra_count = r.u32("vtex extra data count")?;

        let mut texture = Self {
            data,
            data_offset: usize::try_from(block.offset.saturating_add(u64::from(block.size)))
                .unwrap_or(usize::MAX),
            version,
            flags: tex_flags,
            reflectivity,
            width,
            height,
            depth,
            format,
            mip_count,
            picmip0_res,
            non_pow2_width: 0,
            non_pow2_height: 0,
            extra_data: Vec::new(),
            compressed_mip_sizes: None,
            mips_are_compressed: false,
            codec: TextureCodec::default(),
        };

        if extra_count > 0 {
            if extra_count > 64 {
                return Err(Source2Error::InvalidSize {
                    what: "vtex extra data count",
                    value: i64::from(extra_count),
                });
            }
            r.skip_signed(i64::from(extra_offset) - 8, "vtex extra data table")?;
            for _ in 0..extra_count {
                let kind = ExtraDataKind::from_u32(r.u32("vtex extra data kind")?);
                let offset = r.u32("vtex extra data entry offset")?;
                let size = r.u32("vtex extra data entry size")?;
                let resume = r.pos() as u64;

                r.skip_signed(i64::from(offset) - 8, "vtex extra data entry")?;
                let entry_start = r.pos();
                let bytes = r.take(size as usize, "vtex extra data entry")?.to_vec();
                r.seek(entry_start as u64, "vtex extra data entry")?;

                match kind {
                    ExtraDataKind::Metadata => {
                        let _display = r.u16("vtex metadata")?;
                        let nw = r.u16("vtex metadata width")?;
                        let nh = r.u16("vtex metadata height")?;
                        if nw > 0 && nh > 0 && width >= nw && height >= nh {
                            texture.non_pow2_width = nw;
                            texture.non_pow2_height = nh;
                        }
                    }
                    ExtraDataKind::CompressedMipSize => {
                        let compressed = r.u32("vtex compressed mip flag")?;
                        let mips_offset = r.u32("vtex compressed mip offset")?;
                        let mips = r.u32("vtex compressed mip count")?;
                        if mips > 32 {
                            return Err(Source2Error::InvalidSize {
                                what: "vtex compressed mip count",
                                value: i64::from(mips),
                            });
                        }
                        texture.mips_are_compressed = compressed == 1;
                        r.skip_signed(i64::from(mips_offset) - 8, "vtex compressed mip table")?;
                        let mut sizes = Vec::with_capacity(mips as usize);
                        for _ in 0..mips {
                            let size = r.i32("vtex compressed mip size")?;
                            if size < 0 {
                                return Err(Source2Error::InvalidSize {
                                    what: "vtex compressed mip size",
                                    value: i64::from(size),
                                });
                            }
                            #[allow(clippy::cast_sign_loss)]
                            sizes.push(size as u32);
                        }
                        texture.compressed_mip_sizes = Some(sizes);
                    }
                    _ => {}
                }

                texture.extra_data.push(ExtraData { kind, bytes });
                r.seek(resume, "vtex extra data table")?;
            }
        }

        texture.codec = TextureCodec::from_resource(resource, format, texture.is_cube());
        Ok(texture)
    }

    /// Whether the six-faces-per-slice flag is set.
    #[must_use]
    pub fn is_cube(&self) -> bool {
        self.flags & flags::CUBE_TEXTURE != 0
    }

    /// Whether the depth axis shrinks with the mip chain.
    #[must_use]
    pub fn is_volume(&self) -> bool {
        self.flags & flags::VOLUME_TEXTURE != 0
    }

    /// The width to present: the non-power-of-two size when there is one.
    #[must_use]
    pub fn actual_width(&self) -> u16 {
        if self.non_pow2_width > 0 && (self.non_pow2_width != 1 || self.width == 4) {
            self.non_pow2_width
        } else {
            self.width
        }
    }

    /// The height to present.
    #[must_use]
    pub fn actual_height(&self) -> u16 {
        if self.non_pow2_height > 0 && (self.non_pow2_height != 1 || self.height == 4) {
            self.non_pow2_height
        } else {
            self.height
        }
    }

    /// Number of mip levels, never zero.
    #[must_use]
    pub fn mip_levels(&self) -> u32 {
        u32::from(self.mip_count).max(1)
    }

    /// Whether [`Self::decode_mip`] knows this pixel format.
    #[must_use]
    pub fn is_decodable(&self) -> bool {
        matches!(
            self.format,
            TextureFormat::Dxt1
                | TextureFormat::Dxt5
                | TextureFormat::Ati1n
                | TextureFormat::Ati2n
                | TextureFormat::Bc7
                | TextureFormat::Bc6H
                | TextureFormat::Rgba8888
                | TextureFormat::Bgra8888
                | TextureFormat::I8
                | TextureFormat::Ia88
                | TextureFormat::R16
                | TextureFormat::Rg1616
                | TextureFormat::Rgba16161616
        )
    }

    /// Storage size, in bytes, of one complete mip level.
    ///
    /// Mirrors Valve's arithmetic exactly, quirks included, because the mip
    /// chain is addressed by summing these.
    #[must_use]
    pub fn mip_byte_size(&self, level: u32) -> u64 {
        let mut w = u64::from(mip_size(self.width, level));
        let mut h = u64::from(mip_size(self.height, level));
        let mut d = if self.is_volume() {
            u64::from(mip_size(self.depth, level))
        } else {
            u64::from(self.depth)
        };
        if self.is_cube() {
            d *= 6;
        }
        let bytes_per = self.format.block_size() as u64;

        if self.format.is_block_padded() {
            w = w.div_ceil(4) * 4;
            h = h.div_ceil(4) * 4;
            if w < 4 && w > 0 {
                w = 4;
            }
            if h < 4 && h > 0 {
                h = 4;
            }
            if d < 4 && d > 1 {
                d = 4;
            }
            return (w * h / 16) * d * bytes_per;
        }

        w * h * d * bytes_per
    }

    /// File offset and stored length of one mip level's data.
    fn mip_location(&self, level: u32) -> Result<(usize, usize)> {
        let levels = self.mip_levels();
        if level >= levels {
            return Err(Source2Error::MipOutOfRange {
                level,
                count: levels,
            });
        }
        // Mips are stored smallest first, so walk down from the last one.
        let mut offset = self.data_offset as u64;
        for j in (level + 1..levels).rev() {
            offset = offset.saturating_add(self.stored_mip_size(j));
        }
        let stored = self.stored_mip_size(level);
        let start = usize::try_from(offset).map_err(|_| Source2Error::UnexpectedEof {
            what: "vtex mip data",
            needed: 0,
            offset: usize::MAX,
            available: self.data.len(),
        })?;
        let end = start
            .checked_add(usize::try_from(stored).unwrap_or(usize::MAX))
            .ok_or(Source2Error::UnexpectedEof {
                what: "vtex mip data",
                needed: 0,
                offset: start,
                available: self.data.len(),
            })?;
        if end > self.data.len() {
            return Err(Source2Error::UnexpectedEof {
                what: "vtex mip data",
                needed: end - start,
                offset: start,
                available: self.data.len().saturating_sub(start),
            });
        }
        Ok((start, end - start))
    }

    /// Bytes a mip level occupies in the file, which is the compressed run
    /// length when one is recorded and smaller than the plain size.
    fn stored_mip_size(&self, level: u32) -> u64 {
        let plain = self.mip_byte_size(level);
        match &self.compressed_mip_sizes {
            Some(sizes) => match sizes.get(level as usize) {
                Some(&c) => plain.min(u64::from(c)),
                None => plain,
            },
            None => plain,
        }
    }

    /// Raw, decompressed but still format-encoded bytes of one mip level.
    ///
    /// # Errors
    ///
    /// Fails when the level is out of range, the data runs past the end of the
    /// file, or LZ4 decompression fails.
    pub fn mip_bytes(&self, level: u32) -> Result<Vec<u8>> {
        let (start, stored) = self.mip_location(level)?;
        let plain = self.mip_byte_size(level);
        if plain > MAX_MIP_BYTES {
            return Err(Source2Error::AllocationTooLarge {
                what: "vtex mip",
                value: plain,
                limit: MAX_MIP_BYTES,
            });
        }
        #[allow(clippy::cast_possible_truncation)]
        let plain = plain as usize;
        let slice = &self.data[start..start + stored];

        if !self.mips_are_compressed || self.compressed_mip_sizes.is_none() || stored >= plain {
            let mut out = slice[..stored.min(plain)].to_vec();
            out.resize(plain, 0);
            return Ok(out);
        }

        let mut out = vec![0u8; plain];
        let written = lz4_flex::block::decompress_into(slice, &mut out).map_err(|e| {
            Source2Error::Decompress {
                algorithm: "LZ4",
                detail: format!("mip {level}: {e}"),
            }
        })?;
        if written != plain {
            return Err(Source2Error::Decompress {
                algorithm: "LZ4",
                detail: format!("mip {level}: expected {plain} bytes, got {written}"),
            });
        }
        Ok(out)
    }

    /// Decode one mip level of the first face/slice to RGBA8.
    ///
    /// # Errors
    ///
    /// Fails when the level is out of range, the pixel format is not supported
    /// (see [`Self::is_decodable`]), or the stored data is short or malformed.
    pub fn decode_mip(&self, level: u32) -> Result<Image> {
        if !self.is_decodable() {
            return Err(Source2Error::UnsupportedTextureFormat {
                format: self.format.name(),
                value: format!("{:?}", self.format),
            });
        }

        let bytes = self.mip_bytes(level)?;
        // Take the first cube face / array slice.
        let slices = if self.is_cube() {
            6 * u64::from(self.depth).max(1)
        } else {
            u64::from(self.depth).max(1)
        };
        let face_len = if slices > 1 {
            (bytes.len() as u64 / slices) as usize
        } else {
            bytes.len()
        };
        let face = &bytes[..face_len.min(bytes.len())];

        let padded_w = mip_size(self.width, level);
        let padded_h = mip_size(self.height, level);
        let mut image = decode_surface(self.format, padded_w, padded_h, face)?;

        let out_w = mip_size(self.actual_width(), level);
        let out_h = mip_size(self.actual_height(), level);
        if out_w != image.width || out_h != image.height {
            image = crop(&image, out_w, out_h);
        }

        apply_codec(&mut image, self.codec);
        Ok(image)
    }

    /// Decode the largest mip level.
    ///
    /// # Errors
    ///
    /// As [`Self::decode_mip`].
    pub fn decode(&self) -> Result<Image> {
        self.decode_mip(0)
    }
}

/// `max(size >> level, 1)`, as a `u32`.
fn mip_size(size: u16, level: u32) -> u32 {
    (u32::from(size) >> level.min(31)).max(1)
}

fn crop(image: &Image, width: u32, height: u32) -> Image {
    let mut out = Image::new(width, height);
    for y in 0..height.min(image.height) as usize {
        let src = y * image.width as usize * 4;
        let dst = y * width as usize * 4;
        let n = (width.min(image.width) as usize) * 4;
        out.rgba[dst..dst + n].copy_from_slice(&image.rgba[src..src + n]);
    }
    out
}

/// Decode one 2D surface of `format` at `width` x `height`.
fn decode_surface(format: TextureFormat, width: u32, height: u32, data: &[u8]) -> Result<Image> {
    if let Some(block_bytes) = format.bcn_block_bytes() {
        return decode_bcn(format, block_bytes, width, height, data);
    }

    let pixels = (width as usize) * (height as usize);
    let mut image = Image::new(width, height);
    let stride = format.block_size();
    let need = pixels
        .checked_mul(stride)
        .ok_or(Source2Error::AllocationTooLarge {
            what: "vtex surface",
            value: u64::MAX,
            limit: MAX_MIP_BYTES,
        })?;
    if data.len() < need {
        return Err(Source2Error::UnexpectedEof {
            what: "vtex surface",
            needed: need,
            offset: 0,
            available: data.len(),
        });
    }

    for i in 0..pixels {
        let src = &data[i * stride..i * stride + stride];
        let out = &mut image.rgba[i * 4..i * 4 + 4];
        match format {
            TextureFormat::Rgba8888 => out.copy_from_slice(src),
            TextureFormat::Bgra8888 => {
                out[0] = src[2];
                out[1] = src[1];
                out[2] = src[0];
                out[3] = src[3];
            }
            TextureFormat::I8 => {
                out[0] = src[0];
                out[1] = src[0];
                out[2] = src[0];
                out[3] = 255;
            }
            TextureFormat::Ia88 => {
                out[0] = src[0];
                out[1] = src[0];
                out[2] = src[0];
                out[3] = src[1];
            }
            TextureFormat::R16 => {
                out[0] = src[1];
                out[1] = src[1];
                out[2] = src[1];
                out[3] = 255;
            }
            TextureFormat::Rg1616 => {
                out[0] = src[1];
                out[1] = src[3];
                out[2] = 0;
                out[3] = 255;
            }
            TextureFormat::Rgba16161616 => {
                out[0] = src[1];
                out[1] = src[3];
                out[2] = src[5];
                out[3] = src[7];
            }
            _ => {
                return Err(Source2Error::UnsupportedTextureFormat {
                    format: format.name(),
                    value: format!("{format:?}"),
                })
            }
        }
    }
    Ok(image)
}

/// Decode a block-compressed surface, 4x4 block at a time.
fn decode_bcn(
    format: TextureFormat,
    block_bytes: usize,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<Image> {
    let blocks_x = (width as usize).div_ceil(4);
    let blocks_y = (height as usize).div_ceil(4);
    let mut image = Image::new(width, height);

    // Valve's mip sizing pads sub-4-pixel mips to a whole block for some
    // formats but not others, so a short tail is expected rather than fatal.
    let mut owned;
    let data = {
        let need = blocks_x * blocks_y * block_bytes;
        if data.len() < need {
            owned = data.to_vec();
            owned.resize(need, 0);
            &owned[..]
        } else {
            data
        }
    };

    let mut block_rgba = [0u8; 64];
    let mut block_rg = [0u8; 32];
    let mut block_r = [0u8; 16];
    let mut block_f32 = [0f32; 48];

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let src = &data[(by * blocks_x + bx) * block_bytes..][..block_bytes];
            match format {
                TextureFormat::Dxt1 => {
                    bcdec_rs::bc1(src, &mut block_rgba, 16);
                    // Valve treats DXT1 as opaque; the punch-through alpha of
                    // the second BC1 mode is not used by CS2 content.
                    for p in block_rgba.chunks_exact_mut(4) {
                        p[3] = 255;
                    }
                }
                TextureFormat::Dxt5 => bcdec_rs::bc3(src, &mut block_rgba, 16),
                TextureFormat::Bc7 => bcdec_rs::bc7(src, &mut block_rgba, 16),
                TextureFormat::Ati1n => {
                    bcdec_rs::bc4(src, &mut block_r, 4, false);
                    for (i, &r) in block_r.iter().enumerate() {
                        block_rgba[i * 4] = r;
                        block_rgba[i * 4 + 1] = r;
                        block_rgba[i * 4 + 2] = r;
                        block_rgba[i * 4 + 3] = 255;
                    }
                }
                TextureFormat::Ati2n => {
                    bcdec_rs::bc5(src, &mut block_rg, 8, false);
                    for i in 0..16 {
                        let (r, g) = (block_rg[i * 2], block_rg[i * 2 + 1]);
                        block_rgba[i * 4] = r;
                        block_rgba[i * 4 + 1] = g;
                        block_rgba[i * 4 + 2] = reconstruct_z(r, g);
                        block_rgba[i * 4 + 3] = 255;
                    }
                }
                TextureFormat::Bc6H => {
                    bcdec_rs::bc6h_float(src, &mut block_f32, 12, false);
                    for i in 0..16 {
                        for c in 0..3 {
                            block_rgba[i * 4 + c] = linear_to_srgb_byte(block_f32[i * 3 + c]);
                        }
                        block_rgba[i * 4 + 3] = 255;
                    }
                }
                _ => {
                    return Err(Source2Error::UnsupportedTextureFormat {
                        format: format.name(),
                        value: format!("{format:?}"),
                    })
                }
            }

            for row in 0..4usize {
                let y = by * 4 + row;
                if y >= height as usize {
                    break;
                }
                for col in 0..4usize {
                    let x = bx * 4 + col;
                    if x >= width as usize {
                        break;
                    }
                    let dst = (y * width as usize + x) * 4;
                    let src = (row * 4 + col) * 4;
                    image.rgba[dst..dst + 4].copy_from_slice(&block_rgba[src..src + 4]);
                }
            }
        }
    }

    Ok(image)
}

/// Recover the Z of a unit normal whose X and Y are stored as unsigned bytes.
fn reconstruct_z(r: u8, g: u8) -> u8 {
    let x = f32::from(r) / 127.5 - 1.0;
    let y = f32::from(g) / 127.5 - 1.0;
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        ((z * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8
    }
}

fn linear_to_srgb_byte(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Apply the compile-time colour transforms recorded in the edit info.
fn apply_codec(image: &mut Image, codec: TextureCodec) {
    if codec.is_identity() {
        return;
    }
    for p in image.rgba.chunks_exact_mut(4) {
        if codec.dxt5nm {
            p.swap(0, 3);
        }
        if codec.ycocg {
            let mut c = [
                f32::from(p[0]) / 255.0,
                f32::from(p[1]) / 255.0,
                f32::from(p[2]) / 255.0,
            ];
            if codec.srgb {
                for v in &mut c {
                    *v = srgb_to_linear(*v);
                }
            }
            let scale = c[2] * (255.0 / 8.0) + 1.0;
            let co = (c[0] - 128.0 / 255.0) / scale;
            let cg = (c[1] - 128.0 / 255.0) / scale;
            let y = f32::from(p[3]) / 255.0;
            let rgb = [y + co - cg, y + cg, y - co - cg];
            for (i, v) in rgb.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    p[i] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
            p[3] = 255;
        }
        if codec.hemi_oct {
            let nx = (f32::from(p[0]) + f32::from(p[1])) / 255.0 - 1.003_922;
            let ny = (f32::from(p[0]) - f32::from(p[1])) / 255.0;
            let nz = 1.0 - nx.abs() - ny.abs();
            let l = (nx * nx + ny * ny + nz * nz).sqrt().max(f32::EPSILON);
            p[3] = p[2]; // roughness moves to alpha
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                p[0] = ((nx / l * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
                p[1] = ((ny / l * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
                p[2] = ((nz / l * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
        if codec.normalize_normals {
            let sr = i32::from(p[0]) * 2 - 255;
            let sg = i32::from(p[1]) * 2 - 255;
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let db = ((255 * 255 - sr * sr - sg * sg).max(0) as f32).sqrt() as i32;
            p[0] = clamp_u8(sr / 2 + 128);
            p[1] = clamp_u8(sg / 2 + 128);
            p[2] = clamp_u8(db / 2 + 128);
        }
    }
}

fn clamp_u8(v: i32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        v.clamp(0, 255) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::tests::build_resource;

    /// A `vtex_c` DATA block plus its trailing pixel data.
    struct Fixture {
        bytes: Vec<u8>,
    }

    #[allow(clippy::too_many_arguments)]
    fn build_vtex(
        flags: u16,
        width: u16,
        height: u16,
        depth: u16,
        format: u8,
        mip_count: u8,
        compressed_mips: Option<(u32, &[u32])>,
        non_pow2: Option<(u16, u16)>,
        pixels: &[u8],
    ) -> Fixture {
        let mut header = Vec::new();
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&flags.to_le_bytes());
        for _ in 0..4 {
            header.extend_from_slice(&0f32.to_le_bytes());
        }
        header.extend_from_slice(&width.to_le_bytes());
        header.extend_from_slice(&height.to_le_bytes());
        header.extend_from_slice(&depth.to_le_bytes());
        header.push(format);
        header.push(mip_count);
        header.extend_from_slice(&0u32.to_le_bytes()); // picmip0

        let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
        if let Some((w, h)) = non_pow2 {
            let mut body = Vec::new();
            body.extend_from_slice(&0u16.to_le_bytes());
            body.extend_from_slice(&w.to_le_bytes());
            body.extend_from_slice(&h.to_le_bytes());
            entries.push((3, body));
        }
        if let Some((flag, sizes)) = compressed_mips {
            let mut body = Vec::new();
            body.extend_from_slice(&flag.to_le_bytes());
            // The mip table follows the three header words, and the offset is
            // measured from the offset field's own position, so it is 8.
            body.extend_from_slice(&8u32.to_le_bytes());
            body.extend_from_slice(&(sizes.len() as u32).to_le_bytes());
            for s in sizes {
                body.extend_from_slice(&s.to_le_bytes());
            }
            entries.push((4, body));
        }

        if entries.is_empty() {
            header.extend_from_slice(&0u32.to_le_bytes());
            header.extend_from_slice(&0u32.to_le_bytes());
        } else {
            // The table starts right after the two header words we are about
            // to write, so extra_data_offset is 8.
            header.extend_from_slice(&8u32.to_le_bytes());
            header.extend_from_slice(&(entries.len() as u32).to_le_bytes());

            let table_len = entries.len() * 12;
            let mut table = Vec::new();
            let mut bodies = Vec::new();
            for (kind, body) in &entries {
                // Position of this entry's offset field within `header`.
                let offset_field = header.len() + table.len() + 4;
                let body_pos = header.len() + table_len + bodies.len();
                table.extend_from_slice(&kind.to_le_bytes());
                // The offset is measured from its own field position.
                let raw = (body_pos - offset_field) as u32;
                table.extend_from_slice(&raw.to_le_bytes());
                table.extend_from_slice(&(body.len() as u32).to_le_bytes());
                bodies.extend_from_slice(body);
            }
            header.extend_from_slice(&table);
            header.extend_from_slice(&bodies);
        }

        let mut bytes = build_resource(1, &[(b"DATA", header)]);
        bytes.extend_from_slice(pixels);
        Fixture { bytes }
    }

    /// A BC1 block: two endpoint colours and four texels of each index.
    fn bc1_block(c0: u16, c1: u16, indices: u32) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4..8].copy_from_slice(&indices.to_le_bytes());
        b
    }

    fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3)
    }

    #[test]
    fn parses_a_minimal_header() {
        let fixture = build_vtex(0, 4, 4, 1, 1, 1, None, None, &[0u8; 8]);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        assert_eq!(tex.version, 1);
        assert_eq!(tex.width, 4);
        assert_eq!(tex.height, 4);
        assert_eq!(tex.format, TextureFormat::Dxt1);
        assert_eq!(tex.mip_levels(), 1);
        assert_eq!(tex.mip_byte_size(0), 8);
        assert!(tex.compressed_mip_sizes.is_none());
        assert_eq!(tex.actual_width(), 4);
    }

    #[test]
    fn reads_non_pow2_metadata_and_compressed_mip_table() {
        let fixture = build_vtex(
            0,
            8,
            8,
            1,
            1,
            2,
            Some((1, &[8, 24])),
            Some((6, 5)),
            &[0u8; 64],
        );
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        assert_eq!(tex.non_pow2_width, 6);
        assert_eq!(tex.non_pow2_height, 5);
        assert_eq!(tex.actual_width(), 6);
        assert_eq!(tex.actual_height(), 5);
        assert!(tex.mips_are_compressed);
        assert_eq!(tex.compressed_mip_sizes.as_deref(), Some(&[8u32, 24][..]));
        assert_eq!(tex.extra_data.len(), 2);
        assert_eq!(tex.extra_data[0].kind, ExtraDataKind::Metadata);
        assert_eq!(tex.extra_data[1].kind, ExtraDataKind::CompressedMipSize);
    }

    #[test]
    fn rejects_unknown_version() {
        let mut fixture = build_vtex(0, 4, 4, 1, 1, 1, None, None, &[0u8; 8]);
        // The DATA payload starts right after the 16-byte header and the
        // single 12-byte directory entry.
        fixture.bytes[28] = 9;
        let res = Resource::parse(&fixture.bytes).expect("resource");
        assert!(matches!(
            Texture::from_resource(&res),
            Err(Source2Error::UnsupportedTextureVersion { version: 9 })
        ));
    }

    #[test]
    fn decodes_a_hand_built_bc1_block() {
        // c0 > c1 selects the four-colour mode. Endpoints are pure red and
        // pure blue; the index word picks 0,1,2,3 across the first row and
        // then repeats index 0.
        let red = rgb565(255, 0, 0);
        let blue = rgb565(0, 0, 255);
        assert!(red > blue);
        let indices = 0b11_10_01_00;
        let block = bc1_block(red, blue, indices);

        let fixture = build_vtex(0, 4, 4, 1, 1, 1, None, None, &block);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        let image = tex.decode().expect("decode");

        assert_eq!((image.width, image.height), (4, 4));
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 4 + x) * 4;
            [
                image.rgba[i],
                image.rgba[i + 1],
                image.rgba[i + 2],
                image.rgba[i + 3],
            ]
        };
        // Index 0 is endpoint 0 (red), index 1 is endpoint 1 (blue).
        assert_eq!(px(0, 0), [255, 0, 0, 255]);
        assert_eq!(px(1, 0), [0, 0, 255, 255]);
        // Indices 2 and 3 are the two-thirds/one-third blends.
        let two_thirds = px(2, 0);
        let one_third = px(3, 0);
        assert!(two_thirds[0] > one_third[0], "{two_thirds:?} {one_third:?}");
        assert!(two_thirds[2] < one_third[2]);
        assert!(image.is_opaque());
        // Every remaining texel used index 0.
        assert_eq!(px(0, 1), [255, 0, 0, 255]);
        assert_eq!(px(3, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn decodes_rgba8888_and_crops_to_non_pow2() {
        let mut pixels = Vec::new();
        for i in 0..16u8 {
            pixels.extend_from_slice(&[i, i * 2, i * 3, 255]);
        }
        let fixture = build_vtex(0, 4, 4, 1, 4, 1, None, Some((3, 2)), &pixels);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        let image = tex.decode().expect("decode");
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(&image.rgba[0..4], &[0, 0, 0, 255]);
        // Second row starts at source index 4.
        assert_eq!(&image.rgba[12..16], &[4, 8, 12, 255]);
    }

    #[test]
    fn addresses_mips_smallest_first() {
        // Two mips of an 8x8 DXT1 texture: mip 1 (4x4) is 8 bytes and comes
        // first, mip 0 (8x8) is 32 bytes and comes second.
        let mut pixels = vec![0xAAu8; 8];
        pixels.extend_from_slice(&[0x55u8; 32]);
        let fixture = build_vtex(0, 8, 8, 1, 1, 2, None, None, &pixels);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        assert_eq!(tex.mip_byte_size(0), 32);
        assert_eq!(tex.mip_byte_size(1), 8);
        assert_eq!(tex.mip_bytes(1).unwrap(), vec![0xAA; 8]);
        assert_eq!(tex.mip_bytes(0).unwrap(), vec![0x55; 32]);
        assert!(matches!(
            tex.mip_bytes(2),
            Err(Source2Error::MipOutOfRange { .. })
        ));
    }

    #[test]
    fn decompresses_lz4_mips() {
        let plain = vec![0x42u8; 32];
        let compressed = lz4_flex::block::compress(&plain);
        assert!(compressed.len() < plain.len());
        let sizes = [compressed.len() as u32];
        let fixture = build_vtex(0, 8, 8, 1, 1, 1, Some((1, &sizes)), None, &compressed);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        assert_eq!(tex.mip_bytes(0).unwrap(), plain);
    }

    #[test]
    fn uncompressed_flag_reads_the_run_verbatim() {
        let plain = vec![0x11u8; 32];
        let sizes = [32u32];
        let fixture = build_vtex(0, 8, 8, 1, 1, 1, Some((0, &sizes)), None, &plain);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        assert!(!tex.mips_are_compressed);
        assert_eq!(tex.mip_bytes(0).unwrap(), plain);
    }

    #[test]
    fn unsupported_format_reports_rather_than_panics() {
        let fixture = build_vtex(0, 4, 4, 1, 23, 1, None, None, &[0u8; 64]);
        let res = Resource::parse(&fixture.bytes).expect("resource");
        let tex = Texture::from_resource(&res).expect("texture");
        assert_eq!(tex.format, TextureFormat::Etc2);
        assert!(!tex.is_decodable());
        assert!(matches!(
            tex.decode(),
            Err(Source2Error::UnsupportedTextureFormat { .. })
        ));
    }

    #[test]
    fn truncation_never_panics() {
        let fixture = build_vtex(
            0,
            8,
            8,
            1,
            1,
            2,
            Some((1, &[8, 24])),
            Some((6, 5)),
            &[7u8; 64],
        );
        for cut in 0..fixture.bytes.len() {
            if let Ok(res) = Resource::parse(&fixture.bytes[..cut]) {
                if let Ok(tex) = Texture::from_resource(&res) {
                    let _ = tex.decode();
                    let _ = tex.mip_bytes(1);
                    let _ = tex.mip_byte_size(0);
                }
            }
        }
    }

    #[test]
    fn bit_flips_never_panic() {
        let fixture = build_vtex(
            0,
            8,
            8,
            1,
            1,
            2,
            Some((1, &[8, 24])),
            Some((6, 5)),
            &[7u8; 64],
        );
        for i in 0..fixture.bytes.len() {
            for mask in [0x01u8, 0x40, 0xFF] {
                let mut corrupt = fixture.bytes.clone();
                corrupt[i] ^= mask;
                if let Ok(res) = Resource::parse(&corrupt) {
                    if let Ok(tex) = Texture::from_resource(&res) {
                        let _ = tex.decode();
                        for level in 0..4 {
                            let _ = tex.mip_bytes(level);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn box_filter_halves_until_it_fits() {
        let mut image = Image::new(8, 4);
        for (i, p) in image.rgba.chunks_exact_mut(4).enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let v = (i * 4) as u8;
            p.copy_from_slice(&[v, v, v, 255]);
        }
        image.downscale_to(4);
        assert_eq!((image.width, image.height), (4, 2));
        // Top-left output averages source texels 0, 1, 8, 9 -> 0, 4, 32, 36.
        assert_eq!(image.rgba[0], 18);
        image.downscale_to(0);
        assert_eq!((image.width, image.height), (4, 2));
    }

    #[test]
    fn codec_reconstructs_normals() {
        let mut image = Image::new(1, 1);
        image.rgba.copy_from_slice(&[128, 128, 0, 255]);
        apply_codec(
            &mut image,
            TextureCodec {
                normalize_normals: true,
                ..TextureCodec::default()
            },
        );
        // A flat normal stays flat and Z comes back at full strength.
        assert_eq!(image.rgba[0], 128);
        assert_eq!(image.rgba[1], 128);
        assert!(image.rgba[2] > 240, "{}", image.rgba[2]);
    }

    #[test]
    fn codec_swaps_red_and_alpha_for_dxt5nm() {
        let mut image = Image::new(1, 1);
        image.rgba.copy_from_slice(&[10, 128, 30, 200]);
        apply_codec(
            &mut image,
            TextureCodec {
                dxt5nm: true,
                ..TextureCodec::default()
            },
        );
        assert_eq!(image.rgba[0], 200);
        assert_eq!(image.rgba[3], 10);
    }
}
