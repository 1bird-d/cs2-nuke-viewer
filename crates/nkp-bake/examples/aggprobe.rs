//! Scratch tool: what shape are de_nuke's aggregate scene objects, really?
//!
//! mapview's exporter draws 1,482 copies of the pipe kit at the origin. Its
//! typed decoder reads `m_fragmentTransforms` as an array of three four-element
//! rows and falls back to the identity for anything else, so this walks the raw
//! KV3 for every aggregate in the world node and reports what is actually
//! there: how many fragments, whether they carry transforms, and in what
//! encoding.

use anyhow::Result;
use nkp_bake::Resolver;
use source2::{KvValue, Resource};

fn shape(v: &KvValue) -> String {
    match v {
        KvValue::Object(f) => format!(
            "object{{{}}}",
            f.iter()
                .map(|(k, _)| k.as_str())
                .take(6)
                .collect::<Vec<_>>()
                .join(",")
        ),
        KvValue::Array(items) => format!("array[{}] of {}", items.len(), inner(items)),
        KvValue::TypedArray(items) => format!("typed_array[{}] of {}", items.len(), inner(items)),
        KvValue::Binary(b) => format!("binary<{} bytes>", b.len()),
        other => other.type_name().to_string(),
    }
}

fn inner(items: &[KvValue]) -> String {
    items.first().map_or("-".into(), shape)
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .map_or_else(nkp_bake::resolver::default_nuke_vpk, Into::into);
    let resolver = Resolver::open(&path)?;
    println!("mounts: {}", resolver.archive_labels().join(", "));

    let bytes = resolver.read("maps/de_nuke/worldnodes/n0.vwnod_c")?;
    let res = Resource::parse(&bytes)?;
    let doc = res
        .data_kv3()
        .ok_or_else(|| anyhow::anyhow!("no DATA block"))??;

    let aggregates = doc
        .root
        .get("m_aggregateSceneObjects")
        .and_then(KvValue::as_array)
        .unwrap_or(&[]);
    let scene_objects = doc
        .root
        .get("m_sceneObjects")
        .and_then(KvValue::as_array)
        .unwrap_or(&[]);
    println!(
        "\n{} scene objects, {} aggregate scene objects",
        scene_objects.len(),
        aggregates.len()
    );

    // Totals across the whole node, plus a detailed look at the pipe aggregate
    // that mapview collapses.
    let mut total_meshes = 0usize;
    let mut total_fragments = 0usize;
    let mut with_transforms = 0usize;
    let mut has_transform_true = 0usize;
    let mut encodings: Vec<String> = Vec::new();

    for agg in aggregates {
        let model = agg
            .get("m_renderableModel")
            .and_then(KvValue::as_str)
            .unwrap_or("?");
        let meshes = agg
            .get("m_aggregateMeshes")
            .and_then(KvValue::as_array)
            .unwrap_or(&[]);
        let frags = agg
            .get("m_fragmentTransforms")
            .and_then(KvValue::as_array)
            .unwrap_or(&[]);
        total_meshes += meshes.len();
        total_fragments += frags.len();
        if !frags.is_empty() {
            with_transforms += 1;
            let enc = shape(&frags[0]);
            if !encodings.contains(&enc) {
                encodings.push(enc);
            }
        }
        let flagged = meshes
            .iter()
            .filter(|m| m.get("m_bHasTransform").and_then(KvValue::as_bool) == Some(true))
            .count();
        has_transform_true += flagged;

        if model.contains("metal_pipe_001") {
            println!("\n=== {model} ===");
            println!("  m_aggregateMeshes: {}", meshes.len());
            println!("  m_fragmentTransforms: {}", frags.len());
            println!("  meshes with m_bHasTransform: {flagged}");
            if let Some(first) = frags.first() {
                println!("  fragment[0] shape: {}", shape(first));
                println!("  fragment[0] raw:   {first:?}");
            }
            if let Some(first) = meshes.first() {
                println!("  mesh[0]: {}", shape(first));
                for (k, v) in first.as_object().unwrap_or(&[]) {
                    println!("    {k} = {}", shape(v));
                }
            }
            // Draw-call indices tell us whether fragments are sub-ranges.
            let draws: Vec<i64> = meshes
                .iter()
                .filter_map(|m| m.get("m_nDrawCallIndex").and_then(KvValue::as_i64))
                .collect();
            println!(
                "  draw call indices: first {:?} .. last {:?}, distinct {}",
                &draws[..draws.len().min(6)],
                draws.last(),
                draws.iter().collect::<std::collections::HashSet<_>>().len()
            );
        }
    }

    println!("\n--- totals across the node ---");
    println!("aggregate meshes (fragments):     {total_meshes}");
    println!("m_fragmentTransforms entries:     {total_fragments}");
    println!("aggregates with any transform:    {with_transforms}");
    println!("meshes with m_bHasTransform=true: {has_transform_true}");
    println!("fragment transform encodings seen: {encodings:?}");

    // The top aggregates by fragment count are the ones that matter.
    let mut by_size: Vec<(usize, &str)> = aggregates
        .iter()
        .map(|a| {
            (
                a.get("m_aggregateMeshes")
                    .and_then(KvValue::as_array)
                    .map_or(0, <[KvValue]>::len),
                a.get("m_renderableModel")
                    .and_then(KvValue::as_str)
                    .unwrap_or("?"),
            )
        })
        .collect();
    by_size.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    println!("\n--- largest aggregates ---");
    for (n, model) in by_size.iter().take(12) {
        println!("  {n:5}  {}", model.rsplit('/').next().unwrap_or(model));
    }

    Ok(())
}
