//! Fox Engine data formats.

pub mod fox2;
mod names;

pub use fox2::{ContainerKind, DataSet, Entity, Property, Value};
pub use names::NameTable;
