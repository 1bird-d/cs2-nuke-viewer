//! Scratch tool for the cable-tray question: what is `nuke_industrial_props_001`
//! actually made of?
//!
//! The bake merges the kit into aggregate draw calls that no longer carry
//! sub-mesh names. The model resource does. This walks the VPKs directly and
//! prints, per model, every mesh name with its draw calls, triangle counts,
//! materials and local bounds in metres.
//!
//! Sub-commands:
//!   `list <substr>`   every archive entry whose path contains the substring
//!   `model <path>`    mesh/draw-call dump for one .vmdl_c
//!   `mat <path>`      parameter dump for one .vmat_c
//!   `refs <substr>`   every de_nuke-placed model referencing a matching material

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use nkp_bake::Resolver;
use source2::{KvValue, Model, Resource};

const M: f32 = 0.0254;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "list".into());
    let arg = args.next().unwrap_or_default();
    let resolver = Resolver::open(&nkp_bake::resolver::default_nuke_vpk())?;

    match cmd.as_str() {
        "list" => list(&resolver, &arg),
        "model" => model(&resolver, &arg),
        "mat" => mat(&resolver, &arg),
        "refs" => refs(&resolver, &arg),
        "scan" => scan(&resolver, &arg, &args.next().unwrap_or_default()),
        "profile" => profile(&resolver, &arg),
        "tex" => tex(&resolver, &arg, &args.next().unwrap_or_default()),
        "uvrect" => uvrect(&resolver, &arg, &args.next().unwrap_or_default()),
        "place" => place(&resolver, &arg),
        "near" => near(&arg, &args.next().unwrap_or_default()),
        other => {
            eprintln!("unknown command {other}");
            Ok(())
        }
    }
}

fn list(_resolver: &Resolver, needle: &str) -> Result<()> {
    let game = std::path::PathBuf::from(nkp_bake::resolver::DEFAULT_GAME_DIR);
    let archives = [
        nkp_bake::resolver::default_nuke_vpk(),
        game.join("csgo/pak01_dir.vpk"),
        game.join("csgo_imported/pak01_dir.vpk"),
        game.join("csgo_core/pak01_dir.vpk"),
        game.join("core/pak01_dir.vpk"),
    ];
    let mut hits = 0;
    for path in &archives {
        if !path.is_file() {
            continue;
        }
        let Ok(archive) = vpk::Vpk::open(path) else {
            continue;
        };
        let label = path.file_name().unwrap_or_default().to_string_lossy();
        let parent = path
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy();
        for e in archive.entries() {
            if e.path.contains(needle) {
                println!("{parent}/{label:16} {}", e.path);
                hits += 1;
            }
        }
    }
    println!("--- {hits} entries containing {needle:?}");
    Ok(())
}

fn mat(resolver: &Resolver, path: &str) -> Result<()> {
    let bytes = resolver.read(path)?;
    let res = Resource::parse(&bytes)?;
    let doc = res
        .data_kv3()
        .ok_or_else(|| anyhow::anyhow!("no DATA block"))??;
    println!("=== {path} ===");
    dump_kv(&doc.root, 0);
    Ok(())
}

fn dump_kv(v: &KvValue, depth: usize) {
    let pad = "  ".repeat(depth);
    match v {
        KvValue::Object(fields) => {
            for (k, val) in fields {
                match val {
                    KvValue::Object(_) | KvValue::Array(_) | KvValue::TypedArray(_) => {
                        println!("{pad}{k}:");
                        dump_kv(val, depth + 1);
                    }
                    other => println!("{pad}{k} = {}", scalar(other)),
                }
            }
        }
        KvValue::Array(items) | KvValue::TypedArray(items) => {
            for (i, item) in items.iter().enumerate() {
                if depth > 4 {
                    break;
                }
                match item {
                    KvValue::Object(_) | KvValue::Array(_) | KvValue::TypedArray(_) => {
                        println!("{pad}[{i}]");
                        dump_kv(item, depth + 1);
                    }
                    other => println!("{pad}[{i}] = {}", scalar(other)),
                }
            }
        }
        other => println!("{pad}{}", scalar(other)),
    }
}

fn scalar(v: &KvValue) -> String {
    match v {
        KvValue::String(s) => format!("{s:?}"),
        KvValue::Binary(b) => format!("<binary {} bytes>", b.len()),
        other => format!("{other:?}"),
    }
}

fn model(resolver: &Resolver, path: &str) -> Result<()> {
    let bytes = resolver.read(path)?;
    let res = Resource::parse(&bytes)?;
    let m = Model::from_resource(&res)?;
    println!("=== {path} ===");
    println!("{} meshes, lod_masks {}", m.meshes.len(), m.lod_masks.len());
    let mut total_tris = 0usize;
    for (i, mesh) in m.meshes.iter().enumerate() {
        let lod0 = m.mesh_in_lod0(i);
        println!(
            "\n[mesh {i}] {:?}  lod0={lod0}  bounds {:.3} x {:.3} x {:.3} m",
            mesh.name,
            (mesh.max_bounds[0] - mesh.min_bounds[0]) * M,
            (mesh.max_bounds[1] - mesh.min_bounds[1]) * M,
            (mesh.max_bounds[2] - mesh.min_bounds[2]) * M,
        );
        for (p, prim) in mesh.primitives.iter().enumerate() {
            let buf = &mesh.vertex_buffers[prim.vertex_buffer];
            let (mn, mx) = bounds(&buf.positions, &prim.indices);
            let tris = prim.indices.len() / 3;
            if lod0 {
                total_tris += tris;
            }
            println!(
                "   draw {p:3}  {tris:7} tris  {:7.3} x {:7.3} x {:7.3} m  \
                 min[{:8.2},{:8.2},{:8.2}]u  {}",
                (mx[0] - mn[0]) * M,
                (mx[1] - mn[1]) * M,
                (mx[2] - mn[2]) * M,
                mn[0],
                mn[1],
                mn[2],
                prim.material.rsplit('/').next().unwrap_or(&prim.material),
            );
        }
    }
    println!("\ntotal lod0 triangles: {total_tris}");
    Ok(())
}

fn bounds(pos: &[[f32; 3]], idx: &[u32]) -> ([f32; 3], [f32; 3]) {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for &i in idx {
        if let Some(p) = pos.get(i as usize) {
            for a in 0..3 {
                mn[a] = mn[a].min(p[a]);
                mx[a] = mx[a].max(p[a]);
            }
        }
    }
    if mn[0] > mx[0] {
        return ([0.0; 3], [0.0; 3]);
    }
    (mn, mx)
}

/// What else is in the baked scene inside a world box, so a run of geometry can
/// be placed in its surroundings. Box is `x0,y0,z0,x1,y1,z1` in viewer metres.
fn near(scene_path: &str, boxarg: &str) -> Result<()> {
    let b: Vec<f32> = boxarg.split(',').filter_map(|s| s.parse().ok()).collect();
    anyhow::ensure!(b.len() == 6, "want x0,y0,z0,x1,y1,z1");
    let scene = nkp_format::Scene::open(std::path::Path::new(scene_path))?;
    let mut counts: BTreeMap<(String, String), (usize, [f32; 6])> = BTreeMap::new();
    for inst in scene.instances() {
        let overlaps = (0..3).all(|k| {
            inst.aabb_max[k] >= b[k] && inst.aabb_min[k] <= b[k + 3]
        });
        if !overlaps {
            continue;
        }
        let key = (
            scene.instance_name(inst).to_string(),
            scene
                .material_name(&scene.materials()[inst.material as usize])
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string(),
        );
        let e = counts.entry(key).or_insert((
            0,
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
        ));
        e.0 += 1;
        for k in 0..3 {
            e.1[k] = e.1[k].min(inst.aabb_min[k]);
            e.1[k + 3] = e.1[k + 3].max(inst.aabb_max[k]);
        }
    }
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_unstable_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("--- instances overlapping {boxarg} ---");
    for ((name, mat), (n, bb)) in rows.iter().take(60) {
        println!(
            "{n:5}  [{:7.2},{:7.2},{:7.2}]..[{:7.2},{:7.2},{:7.2}]  {name}  {mat}",
            bb[0], bb[1], bb[2], bb[3], bb[4], bb[5]
        );
    }
    Ok(())
}

/// Which de_nuke props put triangles inside a rectangle of the atlas, given in
/// pixels of a 1024-square texture as `x0,y0,x1,y1`. Answers "what is that bit
/// of the texture actually on?".
fn uvrect(resolver: &Resolver, prefix: &str, rect: &str) -> Result<()> {
    let r: Vec<f32> = rect.split(',').filter_map(|s| s.parse().ok()).collect();
    anyhow::ensure!(r.len() == 4, "want x0,y0,x1,y1");
    let game = std::path::PathBuf::from(nkp_bake::resolver::DEFAULT_GAME_DIR);
    let archive = vpk::Vpk::open(game.join("csgo/pak01_dir.vpk"))?;
    let mut hits: Vec<(usize, String)> = Vec::new();
    for e in archive.entries() {
        if !(e.path.starts_with(prefix) && e.path.ends_with(".vmdl_c")) {
            continue;
        }
        let Ok(bytes) = resolver.read(&e.path) else {
            continue;
        };
        let Ok(res) = Resource::parse(&bytes) else {
            continue;
        };
        let Ok(m) = Model::from_resource(&res) else {
            continue;
        };
        let mut n = 0usize;
        for (i, mesh) in m.meshes.iter().enumerate() {
            if !m.mesh_in_lod0(i) {
                continue;
            }
            for prim in &mesh.primitives {
                if !prim.material.contains("nuke_industrial_props_001") {
                    continue;
                }
                let buf = &mesh.vertex_buffers[prim.vertex_buffer];
                for tri in prim.indices.chunks_exact(3) {
                    let mut c = [0.0f32; 2];
                    let mut ok = true;
                    for &v in tri {
                        match buf.uvs.get(v as usize) {
                            Some(uv) => {
                                c[0] += uv[0] * 1024.0 / 3.0;
                                c[1] += uv[1] * 1024.0 / 3.0;
                            }
                            None => ok = false,
                        }
                    }
                    if ok && c[0] >= r[0] && c[0] <= r[2] && c[1] >= r[1] && c[1] <= r[3] {
                        n += 1;
                    }
                }
            }
        }
        if n > 0 {
            hits.push((n, e.path.trim_start_matches(prefix).to_string()));
        }
    }
    hits.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    println!("--- triangles with UV centroid inside {rect} ---");
    for (n, p) in hits.iter().take(40) {
        println!("{n:6}  {p}");
    }
    println!("--- {} models", hits.len());
    Ok(())
}

/// Where in the world the fragments of a matching aggregate actually sit, in
/// the viewer's Y-up metres.
fn place(resolver: &Resolver, needle: &str) -> Result<()> {
    let placements = nkp_bake::world::placements(resolver, "de_nuke")?;
    let mut cache: BTreeMap<String, Vec<(usize, [f32; 3], [f32; 3])>> = BTreeMap::new();
    println!("model  draw  tris   world min (x,y,z) m .. max      dims m");
    let mut n = 0;
    for p in &placements {
        if !p.model.contains(needle) {
            continue;
        }
        let calls = match cache.get(&p.model) {
            Some(c) => c,
            None => {
                let bytes = resolver.read(&p.model)?;
                let res = Resource::parse(&bytes)?;
                let m = Model::from_resource(&res)?;
                let mut v = Vec::new();
                for (i, mesh) in m.meshes.iter().enumerate() {
                    if !m.mesh_in_lod0(i) {
                        continue;
                    }
                    for prim in &mesh.primitives {
                        let buf = &mesh.vertex_buffers[prim.vertex_buffer];
                        let (a, b) = bounds(&buf.positions, &prim.indices);
                        v.push((prim.indices.len() / 3, a, b));
                    }
                }
                cache.entry(p.model.clone()).or_insert(v)
            }
        };
        let Some(&(tris, lmin, lmax)) = p.draw_call.and_then(|i| calls.get(i)) else {
            continue;
        };
        // Eight corners through the Source transform, then Source -> Y-up m.
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for c in 0..8u8 {
            let q = [
                if c & 1 == 0 { lmin[0] } else { lmax[0] },
                if c & 2 == 0 { lmin[1] } else { lmax[1] },
                if c & 4 == 0 { lmin[2] } else { lmax[2] },
            ];
            let s = p.transform.apply(q);
            let w = [s[0] * M, s[2] * M, -s[1] * M];
            for k in 0..3 {
                mn[k] = mn[k].min(w[k]);
                mx[k] = mx[k].max(w[k]);
            }
        }
        println!(
            "{:>44}  {:3}  {tris:6}  [{:7.2},{:7.2},{:7.2}]..[{:7.2},{:7.2},{:7.2}]  \
             {:6.2} x {:6.2} x {:6.2}",
            nkp_bake::geometry::model_stem(&p.model),
            p.draw_call.unwrap_or(0),
            mn[0], mn[1], mn[2], mx[0], mx[1], mx[2],
            mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2],
        );
        n += 1;
    }
    println!("--- {n} placements");
    Ok(())
}

/// Decode a `.vtex_c` to PNG so the atlas can be looked at directly.
fn tex(resolver: &Resolver, path: &str, out: &str) -> Result<()> {
    let bytes = resolver.read(path)?;
    let res = Resource::parse(&bytes)?;
    let t = source2::texture::Texture::from_resource(&res)?;
    println!(
        "{path}: {}x{} {:?} {} mips",
        t.actual_width(),
        t.actual_height(),
        t.format,
        t.mip_levels()
    );
    let img = t.decode()?;
    // Optional `x0,y0,x1,y1[,scale]` crop, so a strip of the atlas can be read
    // at a useful size.
    let crop: Vec<u32> = std::env::args()
        .nth(4)
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    let (mut w, mut h, mut px) = (img.width, img.height, img.rgba.clone());
    if crop.len() >= 4 {
        let (x0, y0, x1, y1) = (crop[0], crop[1], crop[2], crop[3]);
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut out = Vec::with_capacity((cw * ch * 4) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * img.width + x) * 4) as usize;
                out.extend_from_slice(&img.rgba[i..i + 4]);
            }
        }
        w = cw;
        h = ch;
        px = out;
        let scale = crop.get(4).copied().unwrap_or(1).max(1);
        if scale > 1 {
            let (sw, sh) = (w * scale, h * scale);
            let mut up = Vec::with_capacity((sw * sh * 4) as usize);
            for y in 0..sh {
                for x in 0..sw {
                    let i = (((y / scale) * w + (x / scale)) * 4) as usize;
                    up.extend_from_slice(&px[i..i + 4]);
                }
            }
            w = sw;
            h = sh;
            px = up;
        }
    }
    let file = std::fs::File::create(out)?;
    let bw = std::io::BufWriter::new(file);
    let mut enc = png::Encoder::new(bw, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&px)?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}

/// The cross-section of a modular run: every distinct vertex, its UV, and the
/// spacing of repeats along the long axis.
fn profile(resolver: &Resolver, path: &str) -> Result<()> {
    let bytes = resolver.read(path)?;
    let res = Resource::parse(&bytes)?;
    let m = Model::from_resource(&res)?;
    println!("=== {path} ===");
    for (i, mesh) in m.meshes.iter().enumerate() {
        if !m.mesh_in_lod0(i) {
            continue;
        }
        for (p, prim) in mesh.primitives.iter().enumerate() {
            let buf = &mesh.vertex_buffers[prim.vertex_buffer];
            let used: BTreeSet<u32> = prim.indices.iter().copied().collect();
            let (mn, mx) = bounds(&buf.positions, &prim.indices);
            let span = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
            let long = (0..3).max_by(|a, b| span[*a].total_cmp(&span[*b])).unwrap();
            println!(
                "\n[mesh {i} draw {p}] {} verts, {} tris, long axis {}",
                used.len(),
                prim.indices.len() / 3,
                ["X", "Y", "Z"][long]
            );

            // Slice positions along the long axis; a modular kit repeats.
            let mut slices: BTreeMap<i64, usize> = BTreeMap::new();
            for &v in &used {
                let p = buf.positions[v as usize];
                *slices.entry((p[long] * 100.0).round() as i64).or_default() += 1;
            }
            println!("  {} distinct stations on the long axis", slices.len());
            let keys: Vec<f32> = slices.keys().map(|k| *k as f32 / 100.0).collect();
            println!(
                "  stations (units): {}",
                keys.iter()
                    .map(|k| format!("{k:.1}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            // The cross-section: distinct (a, b) of the two short axes.
            let (a, b) = match long {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let mut sect: BTreeMap<(i64, i64), Vec<[f32; 2]>> = BTreeMap::new();
            for &v in &used {
                let p = buf.positions[v as usize];
                let uv = buf.uvs.get(v as usize).copied().unwrap_or([0.0, 0.0]);
                sect.entry(((p[a] * 20.0).round() as i64, (p[b] * 20.0).round() as i64))
                    .or_default()
                    .push(uv);
            }
            println!(
                "  cross-section, {} distinct ({}, {}) points, units, with atlas \
                 pixels at 1024:",
                sect.len(),
                ["X", "Y", "Z"][a],
                ["X", "Y", "Z"][b]
            );
            for ((x, y), uvs) in &sect {
                let px: Vec<String> = uvs
                    .iter()
                    .map(|uv| format!("({:.0},{:.0})", uv[0] * 1024.0, uv[1] * 1024.0))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                println!(
                    "     {:8.2} {:8.2}   {}",
                    *x as f32 / 20.0,
                    *y as f32 / 20.0,
                    px.join(" ")
                );
            }

            // Where the draw call sits in the atlas.
            let mut umin = f32::INFINITY;
            let mut umax = f32::NEG_INFINITY;
            let mut vmin = f32::INFINITY;
            let mut vmax = f32::NEG_INFINITY;
            for &v in &used {
                if let Some(uv) = buf.uvs.get(v as usize) {
                    umin = umin.min(uv[0]);
                    umax = umax.max(uv[0]);
                    vmin = vmin.min(uv[1]);
                    vmax = vmax.max(uv[1]);
                }
            }
            println!("  UV bbox: u {umin:.4}..{umax:.4}  v {vmin:.4}..{vmax:.4}");
        }
    }
    Ok(())
}

/// Every `.vmdl_c` under `prefix` in any archive whose draw calls name a
/// material containing `mat`. This finds the *source* props, not the merged
/// aggregates the map ships.
fn scan(resolver: &Resolver, prefix: &str, mat: &str) -> Result<()> {
    let game = std::path::PathBuf::from(nkp_bake::resolver::DEFAULT_GAME_DIR);
    let archives = [
        nkp_bake::resolver::default_nuke_vpk(),
        game.join("csgo/pak01_dir.vpk"),
        game.join("csgo_imported/pak01_dir.vpk"),
        game.join("csgo_core/pak01_dir.vpk"),
        game.join("core/pak01_dir.vpk"),
    ];
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for a in &archives {
        if !a.is_file() {
            continue;
        }
        let Ok(archive) = vpk::Vpk::open(a) else {
            continue;
        };
        for e in archive.entries() {
            if e.path.starts_with(prefix) && e.path.ends_with(".vmdl_c") {
                paths.insert(e.path.clone());
            }
        }
    }
    println!("{} models under {prefix:?}", paths.len());
    println!("--- those naming a material containing {mat:?} ---");
    let mut hits = 0;
    for path in &paths {
        let Ok(bytes) = resolver.read(path) else {
            continue;
        };
        let Ok(res) = Resource::parse(&bytes) else {
            continue;
        };
        let Ok(m) = Model::from_resource(&res) else {
            continue;
        };
        let mut tris = 0usize;
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut uv = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for (i, mesh) in m.meshes.iter().enumerate() {
            if !m.mesh_in_lod0(i) {
                continue;
            }
            for prim in &mesh.primitives {
                if !prim.material.contains(mat) {
                    continue;
                }
                tris += prim.indices.len() / 3;
                names.insert(mesh.name.clone());
                let buf = &mesh.vertex_buffers[prim.vertex_buffer];
                let (a, b) = bounds(&buf.positions, &prim.indices);
                for k in 0..3 {
                    mn[k] = mn[k].min(a[k]);
                    mx[k] = mx[k].max(b[k]);
                }
                for &v in prim.indices.iter() {
                    if let Some(t) = buf.uvs.get(v as usize) {
                        uv[0] = uv[0].min(t[0]);
                        uv[1] = uv[1].min(t[1]);
                        uv[2] = uv[2].max(t[0]);
                        uv[3] = uv[3].max(t[1]);
                    }
                }
            }
        }
        if tris > 0 {
            hits += 1;
            println!(
                "{tris:7} tris  {:7.3} x {:7.3} x {:7.3} m  uv px [{:5.0},{:5.0}]..[{:5.0},{:5.0}]  {}",
                (mx[0] - mn[0]) * M,
                (mx[1] - mn[1]) * M,
                (mx[2] - mn[2]) * M,
                uv[0] * 1024.0,
                uv[1] * 1024.0,
                uv[2] * 1024.0,
                uv[3] * 1024.0,
                path.trim_start_matches(prefix),
            );
        }
    }
    println!("--- {hits} models");
    Ok(())
}

/// Every model placed in de_nuke that names a material matching `needle`,
/// with the placement count and triangle total attributable to it.
fn refs(resolver: &Resolver, needle: &str) -> Result<()> {
    let placements = nkp_bake::world::placements(resolver, "de_nuke")?;
    let mut models: BTreeMap<String, usize> = BTreeMap::new();
    for p in &placements {
        *models.entry(p.model.clone()).or_default() += 1;
    }
    println!("{} placements over {} models", placements.len(), models.len());

    let mut matched: BTreeMap<String, (usize, usize, BTreeSet<String>)> = BTreeMap::new();
    for (path, count) in &models {
        let Ok(bytes) = resolver.read(path) else {
            continue;
        };
        let Ok(res) = Resource::parse(&bytes) else {
            continue;
        };
        let Ok(m) = Model::from_resource(&res) else {
            continue;
        };
        let mut tris = 0usize;
        let mut mats = BTreeSet::new();
        for (i, mesh) in m.meshes.iter().enumerate() {
            if !m.mesh_in_lod0(i) {
                continue;
            }
            for prim in &mesh.primitives {
                if prim.material.contains(needle) {
                    tris += prim.indices.len() / 3;
                    mats.insert(prim.material.clone());
                }
            }
        }
        if tris > 0 {
            matched.insert(path.clone(), (*count, tris, mats));
        }
    }
    println!("\n--- models naming a material containing {needle:?} ---");
    for (path, (count, tris, mats)) in &matched {
        println!(
            "{count:5} placements  {tris:8} tris  {}\n            {:?}",
            path,
            mats.iter().collect::<Vec<_>>()
        );
    }
    Ok(())
}
