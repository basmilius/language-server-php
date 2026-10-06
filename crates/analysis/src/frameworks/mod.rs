//! What Laravel and Symfony add that the type layer cannot read from declarations alone: strings that
//! name config keys, routes, views and services, and the rest of what the frameworks make up at run
//! time. The members that Eloquent and the facades make up live in `php-index`, which is where the
//! type layer finds members.

pub(crate) mod complete;
mod container;
pub mod keys;
pub mod relations;
pub mod rules;

pub use complete::complete_key;

#[cfg(test)]
mod tests;
