//! A tiny library that the Rust test project is built from.
//!
//! `main.rs` links against this crate, so the cross-file reference to [`Person`]
//! gives `definition`, `references`, `hover`, `workspaceSymbol`, and `rename`
//! something to act on.

/// A person we can greet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The person's display name.
    pub name: String,
    /// The person's age in whole years.
    pub age: u8,
}

impl Person {
    /// Builds a [`Person`] from a `name` and an `age`.
    pub fn new(name: impl Into<String>, age: u8) -> Self {
        Self {
            name: name.into(),
            age,
        }
    }

    /// Greets this person by name.
    pub fn greet(&self) -> String {
        greet(&self.name)
    }
}

/// Returns a greeting for `name`.
pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_by_name() {
        assert_eq!(greet("world"), "Hello, world!");
        assert_eq!(Person::new("Ada", 36).greet(), "Hello, Ada!");
    }
}
