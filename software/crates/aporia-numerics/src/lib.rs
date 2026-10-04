//! # aporia-numerics
//!
//! The numerical side of APORIA: the tools that turn "these two runs gave different answers" into a
//! statement about the model rather than about the machine.
//!
//! - [`agree`] — ulp, relative and absolute distance, and cancellation measured in digits
//! - [`dd`] — double-double arithmetic, roughly 106 bits of mantissa in two f64s
//! - [`reference`] — evaluating a whole model in double-double, as an independent comparator
//! - [`rng`] — the deterministic generator every sampler in the project draws from
//!
//! [`reference`] deliberately does not share code with the runtime's arithmetic. Two paths that use
//! the same operations cannot tell you anything; the value of a comparator is that it reached its
//! answer a different way.
pub mod agree;
pub mod dd;
pub mod reference;
pub mod rng;

pub use agree::{Distance, cancellation, relative, ulps};
pub use dd::Dd;
pub use reference::Reference;
pub use rng::Rng;
