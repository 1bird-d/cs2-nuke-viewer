//! Reads de_nuke out of the CS2 archives and bakes it into a `.nkp` scene.
//!
//! The viewer wants a layout the VPK does not hand you: concatenated vertex and
//! index buffers, a flat instance array, world bounds per instance, and a
//! connectivity graph over the pipework. Deriving all that means decompressing
//! a quarter-gigabyte archive and every model in it, so it happens once here
//! and the viewer memory-maps the result.
//!
//! Decoding of the Source 2 containers themselves — VPK, KV3, VBIB/MBUF,
//! meshopt, BCn — is mapview's, used as a read-only dependency. The one thing
//! this crate does *not* borrow is mapview's world-node decoder; see
//! [`world`] for why.

pub mod geometry;
pub mod resolver;
pub mod world;

pub use resolver::Resolver;
