//! # aporia-boundary
//!
//! The Trust Atlas. An adaptive partition of the parameter space ([`atlas`]), the thresholds that
//! turn cell statistics into a label, and the boundary bands read out of the finished partition.
//!
//! The representation is a recursive bisection rather than a mesh or an implicit surface: it labels
//! and prints in any dimension, it refines locally so resolution follows evidence, and in one
//! dimension it degenerates into the interval search that the boundary-precision metric can be
//! checked against. What it costs is that a curved transition needs many cells, and the atlas
//! reports cell counts so that cost is visible instead of hidden.
//!
//! Nothing here calls `TRUSTED` a proof. A trusted cell is one that was sampled enough, was measured
//! by enough channels, and saw nothing. Since E1.4 it is also one that was looked at closely enough:
//! a quiet cell coarser than [`Resolution`]'s certification scale is `PROVISIONAL`, which is neither a
//! claim about the space nor an admission of not having measured it.

mod atlas;

pub use atlas::{Atlas, Band, Cell, Coverage, FACE_EPS, Label, Point, Policy, Resolution, Trust};
