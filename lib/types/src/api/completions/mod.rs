use serde::{Deserialize, Serialize};

use crate::jotoba::{search::QueryType, words::Word};

/// Request payload structure for suggestion endpoint
#[derive(Deserialize)]
pub struct Request {
    /// The search query to find suggestions for
    pub input: String,

    /// The user configured language
    #[serde(default)]
    pub lang: String,

    /// The search type the input is designed for
    #[serde(default)]
    pub search_type: QueryType,
}

/// Response struct for suggestion endpoint
#[derive(Serialize, Default)]
pub struct Response {
    pub suggestions: Vec<WordPair>,
    pub suggestion_type: SuggestionType,
}

/// The type of suggestion. `Default` in most cases
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionType {
    /// Default suggestion type
    Default,
    /// Special suggestion type for kanji readings
    KanjiReading,
}

impl Default for SuggestionType {
    #[inline]
    fn default() -> Self {
        Self::Default
    }
}

/// A word with kana and kanji reading used within [`SuggestionResponse`]
#[derive(Serialize, Default, PartialEq, Eq, Debug, Hash, Clone)]
pub struct WordPair {
    pub primary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<String>,
}

#[cfg(feature = "jotoba_intern")]
impl WordPair {
    /// Returns true if [`self`] contains [`reading`]
    pub fn has_reading(&self, reading: &str) -> bool {
        self.primary == reading
            || self
                .secondary
                .as_ref()
                .map(|i| i == reading)
                .unwrap_or_default()
    }
}

#[cfg(feature = "jotoba_intern")]
impl From<&Word> for WordPair {
    #[inline]
    fn from(word: &Word) -> Self {
        let main_reading = word.get_reading().reading.to_owned();
        if word.reading.kanji.is_some() {
            WordPair {
                secondary: Some(main_reading),
                primary: word.reading.kana.reading.clone(),
            }
        } else {
            WordPair {
                primary: main_reading,
                secondary: None,
            }
        }
    }
}