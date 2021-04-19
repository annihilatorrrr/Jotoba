#![allow(dead_code)]

pub mod everything;
pub mod kanji;
pub mod result;
mod result_order;
pub mod word;

pub use self::result::Item as ResultItem;

/// How db entries should be matched with
/// the query in order to be valid as result
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SearchMode {
    Exact,
    Variable,
    LeftVariable,
    RightVariable,
}

impl SearchMode {
    /// Compares a string based on the mode and case
    pub fn str_eq<S: AsRef<str>>(&self, a: S, b: S, ign_case: bool) -> bool {
        let (a, b) = if ign_case {
            (a.as_ref().to_lowercase(), b.as_ref().to_lowercase())
        } else {
            (a.as_ref().to_owned(), b.as_ref().to_owned())
        };

        match *self {
            SearchMode::Exact => a == b,
            SearchMode::Variable => a.contains(&b),
            SearchMode::LeftVariable => a.starts_with(&b),
            SearchMode::RightVariable => a.ends_with(&b),
        }
    }
}

/// Predefines data, required for
/// each type of search
#[derive(Clone, PartialEq, Debug)]
pub struct Search<'a> {
    pub query: &'a str,
    pub limit: u16,
    pub mode: SearchMode,
}

impl<'a> Search<'a> {
    pub fn new(query: &'a str, mode: SearchMode) -> Self {
        Self {
            query,
            limit: 0,
            mode,
        }
    }

    /// Add a limit to the search
    pub fn with_limit(&mut self, limit: u16) -> &mut Self {
        self.limit = limit;
        self
    }
}
