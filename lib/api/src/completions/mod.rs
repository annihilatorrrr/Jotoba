mod kanji;
mod names;
mod storage;
mod words;

pub use storage::load_suggestions;

use std::{cmp::Ordering, str::FromStr};

use config::Config;
use error::api_error::RestError;
use japanese::JapaneseExt;
use query_parser::{QueryParser, QueryType};
use resources::{models, parse::jmdict::languages::Language};
use search::{
    query::{Form, Query, QueryLang, UserSettings},
    query_parser,
};
use utils::{bool_ord, real_string_len};

use actix_web::{
    rt::time,
    web::{self, Json},
};
use serde::{Deserialize, Serialize};

/// Request struct for suggestion endpoint
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

impl Request {
    /// Adjust the query and returns a newly allocated one
    fn adjust(&self) -> Self {
        let mut query_str = self.input.as_str();
        let query_len = real_string_len(&self.input);

        // Some inputs place the roman letter of the japanese text while typing with romanized input.
        // If input is japanese but last character is a romanized letter, strip it off
        let last_char = query_str.chars().rev().next().unwrap();
        if query_parser::parse_language(query_str) == QueryLang::Japanese
            && last_char.is_roman_letter()
            && query_len > 1
        {
            query_str = &query_str[..query_str.bytes().count() - last_char.len_utf8()];
        }

        Self {
            input: query_str.to_owned(),
            lang: self.lang.to_owned(),
            search_type: self.search_type,
        }
    }

    // Returns a [`Query`] based on the [`Request`]
    fn get_query(&self) -> Result<Query, RestError> {
        let query_str = self.input.trim_start().to_string();

        let search_type = self.search_type;

        let settings = UserSettings {
            user_lang: self.get_language(),
            ..UserSettings::default()
        };

        // Build and parse the query
        let query = QueryParser::new(query_str, search_type, settings, 0, 0, false)
            .parse()
            .ok_or(RestError::BadRequest)?;

        Ok(query)
    }

    // Returns the user configured language of the [`Request`]
    #[inline]
    fn get_language(&self) -> Language {
        Language::from_str(&self.lang).unwrap_or_default()
    }
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
    fn default() -> Self {
        Self::Default
    }
}

/// A word with kana and kanji reading used within [`SuggestionResponse`]
#[derive(Serialize, Default, PartialEq, Eq)]
pub struct WordPair {
    pub primary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<String>,
}

impl WordPair {
    /// Returns true if [`self`] contains [`reading`]
    fn has_reading(&self, reading: &str) -> bool {
        self.primary == reading
            || self
                .secondary
                .as_ref()
                .map(|i| i == reading)
                .unwrap_or_default()
    }
}

/// Get search suggestions endpoint
pub async fn suggestion_ep(
    config: web::Data<Config>,
    payload: Json<Request>,
) -> Result<Json<Response>, actix_web::Error> {
    validate_request(&payload)?;

    // Adjust payload and parse to query
    let query = payload.adjust().get_query()?;

    // time we allow the suggestion to use in total loaded from the configuration file
    let timeout = config.get_suggestion_timeout();

    let result = time::timeout(timeout, get_suggestions(query))
        .await
        .map_err(|_| RestError::Timeout)??;

    Ok(Json(result))
}

/// Returns best matching suggestions for the given query
async fn get_suggestions(query: Query) -> Result<Response, RestError> {
    match query.type_ {
        QueryType::Sentences | QueryType::Words => {
            if let Some(kanji_reading) = as_kanji_reading(&query) {
                kanji::reading::suggestions(kanji_reading).await
            } else {
                Ok(get_word_suggestions(query).await.unwrap_or_default())
            }
        }
        QueryType::Kanji => kanji_suggestions(query).await,
        QueryType::Names => name_suggestions(query).await,
    }
}

/// Returns name suggestions for the matching input language
async fn name_suggestions(query: Query) -> Result<Response, RestError> {
    Ok(match query.language {
        QueryLang::Japanese => names::native_suggestions(&query).await?,
        QueryLang::Foreign => names::transcription_suggestions(&query).await?,
        _ => Response::default(),
    })
}

/// Returns kanji suggestions
async fn kanji_suggestions(query: Query) -> Result<Response, RestError> {
    if query.language == QueryLang::Foreign {
        kanji::meaning::suggestions(&query).await
    } else {
        Ok(Response::default())
    }
}

/// Returns Some(KanjiReading) if query is or 'could be' a kanji reading query
fn as_kanji_reading(query: &Query) -> Option<models::kanji::Reading> {
    match &query.form {
        Form::KanjiReading(r) => Some(r.clone()),
        _ => {
            let mut query_str = query.original_query.chars();
            let first = query_str.next()?;
            let second = query_str.next()?;

            if first.is_kanji() && second == ' ' {
                Some(models::kanji::Reading {
                    reading: String::new(),
                    literal: first,
                })
            } else {
                None
            }
        }
    }
}

/// Returns word suggestions based on the query. Applies various approaches to give better results
async fn get_word_suggestions(query: Query) -> Option<Response> {
    let response = try_word_suggestions(&query).await?;

    // Tries to do a katakana search if nothing was found
    let result = if response.is_empty() && query.query.is_hiragana() {
        try_word_suggestions(&get_katakana_query(&query)).await?
    } else {
        response
    };

    Some(Response {
        suggestions: result,
        ..Default::default()
    })
}

/// Returns Ok(suggestions) for the given query ordered and ready to display
async fn try_word_suggestions(query: &Query) -> Option<Vec<WordPair>> {
    // Get sugesstions for matching language
    let mut word_pairs = match query.language {
        QueryLang::Japanese => words::native::suggestions(&query.query).await?,
        QueryLang::Foreign | QueryLang::Undetected => {
            words::foreign::suggestions(&query, &query.query)
                .await
                .unwrap_or_default()
        }
    };

    // Order: put exact matches to top
    word_pairs.sort_by(|a, b| word_pair_order(a, b, &query.query));

    Some(word_pairs)
}

/// Ordering for [`WordPair`]s which puts the exact matches to top
fn word_pair_order(a: &WordPair, b: &WordPair, query: &str) -> Ordering {
    bool_ord(a.has_reading(&query), b.has_reading(&query))
}

/// Returns an equivalent katakana query
fn get_katakana_query(query: &Query) -> Query {
    Query {
        query: romaji::RomajiExt::to_katakana(query.query.as_str()),
        ..query.clone()
    }
}

/// Validates the API request payload
fn validate_request(payload: &Request) -> Result<(), RestError> {
    let query_len = real_string_len(&payload.input);
    if query_len < 1 || query_len > 37 {
        return Err(RestError::BadRequest.into());
    }

    Ok(())
}
