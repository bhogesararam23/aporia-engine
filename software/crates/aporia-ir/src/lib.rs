//! # A-IR
//!
//! The intermediate representation at the centre of APORIA. A scientific model, once parsed or
//! adapted, exists here and only here: the runtime, the vector and GPU backends, the property
//! analyses and the report writer all read this shape.
//!
//! The object vocabulary follows the specification: **Parameter**, **Expression**, **Constraint**,
//! **Relation**, **Observation**, **Experiment**, **Evidence**, **Boundary**. Of those, A-IR carries
//! the five that describe a computation — parameters, expressions (instructions), constraints,
//! relations, and the declarations that name observed quantities — plus the structure a
//! declaration needs to be replayable. Observations, evidence and boundaries are produced by
//! running a model, so they live in the crates that produce them and refer into A-IR by index.
//!
//! Nothing in this crate executes anything, and nothing outside `aporia-dsl` should construct a
//! `Model` by hand except tests.

mod dim;
mod ir;

pub use dim::{AMOUNT, CURRENT, Dimension, LENGTH, LUMINOUS, MASS, NumType, TEMPERATURE, TIME, Ty};
pub use ir::{
    Binop, Block, BlockId, Builtin, CmpOp, Constraint, ConstraintId, ConstraintKind, Direction,
    Domain, Id, Instr, InstrKind, Lit, Model, Operand, Origin, Output, OutputId, Param, ParamId,
    Relation, RelationId, RelationKind, Slot, SlotId, Trace, TraceId, Unop,
};
