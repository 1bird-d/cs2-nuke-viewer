//! Turns `de_nuke.vpk` into a `.nkp` scene the viewer can memory-map.
//!
//! Reads the CS2 install; writes only the output file. Neither the game nor
//! mapview is ever modified.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use nkp_bake::{geometry, world, Resolver};

#[derive(Parser)]
#[command(name = "nkp-bake", about = "Bake a CS2 map into a .nkp scene")]
struct Args {
    /// Map archive. Defaults to de_nuke in the standard Steam install.
    #[arg(long)]
    vpk: Option<PathBuf>,

    /// Map name inside the archive.
    #[arg(long, default_value = "de_nuke")]
    map: String,

    /// Where to write the scene.
    #[arg(short, long, default_value = "scenes/de_nuke.nkp")]
    out: PathBuf,

    /// Report what would be baked without writing anything.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let vpk = args.vpk.unwrap_or_else(nkp_bake::resolver::default_nuke_vpk);

    let started = Instant::now();
    let resolver = Resolver::open(&vpk)?;
    println!("mounted: {}", resolver.archive_labels().join(", "));

    let placements = world::placements(&resolver, &args.map)?;
    report_placements(&placements);

    let scene = geometry::bake(&resolver, &placements)?;
    report_scene(&scene, &placements);

    if args.dry_run {
        println!("\ndry run: nothing written");
        return Ok(());
    }

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let bytes = geometry::to_bytes(&scene);
    std::fs::write(&args.out, &bytes)
        .with_context(|| format!("writing {}", args.out.display()))?;

    #[allow(clippy::cast_precision_loss)]
    let mb = bytes.len() as f64 / (1024.0 * 1024.0);
    println!(
        "\nwrote {} ({mb:.1} MB) in {:.1}s",
        args.out.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

fn report_placements(placements: &[world::Placement]) {
    let scene_objects = placements
        .iter()
        .filter(|p| p.kind == nkp_format::flags::SCENE_OBJECT)
        .count();
    let agg_prop = placements
        .iter()
        .filter(|p| p.kind == nkp_format::flags::AGG_PROP)
        .count();
    let agg_merge = placements
        .iter()
        .filter(|p| p.kind == nkp_format::flags::AGG_MERGE)
        .count();
    let placed = placements.iter().filter(|p| !p.transform.is_identity()).count();
    println!(
        "placements: {} total ({scene_objects} scene, {agg_prop} aggregate fragments, \
         {agg_merge} merged draw calls), {placed} with a non-identity transform",
        placements.len()
    );
}

fn report_scene(scene: &geometry::BakedScene, placements: &[world::Placement]) {
    println!(
        "geometry:   {} vertices, {} triangles, {} instances, {} materials",
        scene.vertices.len(),
        scene.indices.len() / 3,
        scene.instances.len(),
        scene.materials.len()
    );
    println!(
        "bounds:     [{:.1}, {:.1}, {:.1}] .. [{:.1}, {:.1}, {:.1}] metres",
        scene.scene_min[0],
        scene.scene_min[1],
        scene.scene_min[2],
        scene.scene_max[0],
        scene.scene_max[1],
        scene.scene_max[2]
    );

    if !scene.failed_models.is_empty() {
        println!("\n{} models failed to decode:", scene.failed_models.len());
        for failure in scene.failed_models.iter().take(10) {
            println!("  {failure}");
        }
    }

    // The point of the whole project: the pipe kit must land in 1,482 different
    // places, not stacked on the origin. Report it every run so a regression in
    // the transform decode is impossible to miss.
    let mut by_stem: HashMap<&str, Vec<[f32; 3]>> = HashMap::new();
    for placement in placements {
        let stem = geometry::model_stem(&placement.model);
        by_stem
            .entry(stem)
            .or_default()
            .push([
                placement.transform.0[0][3],
                placement.transform.0[1][3],
                placement.transform.0[2][3],
            ]);
    }
    let mut biggest: Vec<(&str, &Vec<[f32; 3]>)> = by_stem.iter().map(|(k, v)| (*k, v)).collect();
    biggest.sort_unstable_by_key(|(_, v)| std::cmp::Reverse(v.len()));

    println!("\nplacement spread of the largest aggregates:");
    for (stem, origins) in biggest.iter().take(8) {
        let distinct: std::collections::HashSet<[u32; 3]> = origins
            .iter()
            .map(|o| [o[0].to_bits(), o[1].to_bits(), o[2].to_bits()])
            .collect();
        let span = spread(origins);
        println!(
            "  {:5} placements, {:5} distinct origins, span {:.1} x {:.1} x {:.1} units  {stem}",
            origins.len(),
            distinct.len(),
            span[0],
            span[1],
            span[2]
        );
    }
}

fn spread(origins: &[[f32; 3]]) -> [f32; 3] {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for o in origins {
        for axis in 0..3 {
            min[axis] = min[axis].min(o[axis]);
            max[axis] = max[axis].max(o[axis]);
        }
    }
    [max[0] - min[0], max[1] - min[1], max[2] - min[2]]
}
