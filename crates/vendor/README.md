# Vendored code

`vpk` and `source2` are **not** nukeplant's work. They were written for the
[mapview](https://github.com/example/mapview) project and are copied here
unmodified, under that project's `MIT OR Apache-2.0` licence.

`vpk` reads Valve VPK archives. `source2` decodes compiled Source 2 resources —
binary KeyValues3, VBIB/MBUF vertex buffers, meshopt compression, BCn textures.
Between them they are what lets nukeplant read a CS2 map out of your own game
install. Reimplementing either would have been weeks of work for no gain.

## Why they are copied rather than depended on

They were path dependencies into a sibling checkout of mapview:

```toml
vpk = { path = "../mapview/crates/vpk" }
```

That works on one machine and nowhere else, so a clone of this repository could
not be built by anyone. The project README named vendoring as the fallback from
the beginning; publishing the repository is what made it necessary.

## What was changed

Only the two `Cargo.toml` files, and only to drop a `repository.workspace = true`
key that this workspace does not define. **No Rust source was modified.** If you
want to diff them against the originals, that is the point — they should match.

## What nukeplant does *not* use

`source2::world`, the typed world-node decoder, is deliberately unused. It reads
a transform as three nested rows of four, which is right for `m_vTransform` on a
scene object and wrong for `m_fragmentTransforms` on an aggregate, where the
twelve values are a flat array of doubles. The row-wise reader finds a `Double`
where it wants an array and falls back to the identity matrix, which collapses
4,748 de_nuke placements — including every one of the 1,482 pipes — onto the
world origin.

`crates/nkp-bake/src/world.rs` decodes the raw KV3 itself and accepts both
encodings. That bug is the reason this project exists as something other than a
glTF loader, and it is upstream's to fix, not ours: these files are a copy, and
patching a copy would only make the next sync harder.
