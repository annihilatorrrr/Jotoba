mod kanji;
mod order;
pub mod result;
mod wordsearch;

use deadpool_postgres::Pool;
use parse::jmdict::part_of_speech::PosSimple;
use result::{Item, Word};
pub use wordsearch::WordSearch;

use async_std::sync::Mutex;
use itertools::Itertools;
use once_cell::sync::Lazy;

use crate::query::Tag;

use super::{
    query::{Query, QueryLang},
    search_order::SearchOrder,
};
use cache::SharedCache;
use error::Error;
use japanese::{inflection::SentencePart, JapaneseExt};
use models::kanji::KanjiResult;
use models::search_mode::SearchMode;
use utils::real_string_len;

use self::result::{InflectionInformation, WordResult};

#[cfg(feature = "tokenizer")]
use japanese::jp_parsing::{igo_unidic::WordClass, InputTextParser, ParseResult, WordItem};

use super::query::Form;

/// An in memory Cache for word search results
static SEARCH_CACHE: Lazy<Mutex<SharedCache<u64, WordResult>>> =
    Lazy::new(|| Mutex::new(SharedCache::with_capacity(1000)));

pub(self) struct Search<'a> {
    pool: &'a Pool,
    query: &'a Query,
}

/// Search among all data based on the input query
pub async fn search(pool: &Pool, query: &Query) -> Result<WordResult, Error> {
    let search = Search { pool, query };

    // Try to use cache
    if let Some(c_res) = search.get_cache().await {
        return Ok(c_res);
    }

    // Perform the search
    let results = search.do_search().await?;

    search.save_cache(results.clone()).await;
    Ok(results)
}

#[derive(Default)]
pub(crate) struct ResultData {
    pub(crate) words: Vec<Word>,
    pub(crate) infl_info: Option<InflectionInformation>,
    pub(crate) count: usize,
    pub(crate) sentence_index: i32,
    pub(crate) sentence_parts: Option<Vec<SentencePart>>,
    pub(crate) searched_query: String,
}

impl<'a> Search<'a> {
    /// Do the search
    async fn do_search(&self) -> Result<WordResult, Error> {
        let search_result = match self.query.form {
            Form::KanjiReading(_) => kanji::by_reading(self).await?,
            _ => self.do_word_search().await?,
        };

        // Load collocations
        let mut words = search_result.words;

        let words_cloned = words.clone();

        let (kanji_results, _): (Vec<KanjiResult>, _) = futures::try_join!(
            kanji::load_word_kanji_info(&self, &words_cloned),
            WordSearch::load_collocations(self.pool, &mut words, self.query.settings.user_lang)
        )?;

        let kanji_items = kanji_results.len();
        return Ok(WordResult {
            items: Self::merge_words_with_kanji(words, kanji_results),
            contains_kanji: kanji_items > 0,
            inflection_info: search_result.infl_info,
            count: search_result.count,
            sentence_parts: search_result.sentence_parts,
            sentence_index: search_result.sentence_index,
            searched_query: search_result.searched_query,
        });
    }

    /// Search by a word
    async fn do_word_search(&self) -> Result<ResultData, Error> {
        // Perform searches asynchronously
        let (native_word_res, gloss_word_res): (ResultData, ResultData) =
            futures::try_join!(self.native_results(&self.query.query), self.gloss_results())?;

        let sentence_parts = native_word_res
            .sentence_parts
            .map(|i| Some(i))
            .unwrap_or(gloss_word_res.sentence_parts);

        // Chain native and word results into one vector
        Ok(ResultData {
            words: native_word_res
                .words
                .into_iter()
                .chain(gloss_word_res.words)
                .collect_vec(),
            infl_info: native_word_res.infl_info,
            count: native_word_res.count + gloss_word_res.count,
            sentence_parts,
            sentence_index: self.query.word_index as i32,
            searched_query: native_word_res.searched_query,
        })
    }

    #[cfg(not(feature = "tokenizer"))]
    fn get_query(&self, query_str: &str) -> (String, Option<Vec<SentencePart>>) {
        (query_str.to_owned(), None)
    }

    #[cfg(feature = "tokenizer")]
    async fn get_query<'b>(
        &'b self,
        query_str: &'b str,
    ) -> Result<
        (
            String,
            Option<WordItem<'static, 'b>>,
            Option<Vec<SentencePart>>,
        ),
        Error,
    > {
        if !self.query.parse_japanese {
            return Ok((query_str.to_owned(), None, None));
        }

        let in_db = models::dict::reading_existsv2(&self.pool, query_str).await?;
        let parser = InputTextParser::new(query_str, &japanese::jp_parsing::JA_NL_PARSER, in_db)?;

        if let Some(parsed) = parser.parse() {
            if parsed.items.is_empty() {
                return Ok((query_str.to_owned(), None, None));
            }

            let index = self.query.word_index.clamp(0, parsed.items.len() - 1);
            let res = parsed.items[index].clone();
            let sentence = Self::format_setence_parts(self, parsed).await;

            Ok((res.get_lexeme().to_string(), Some(res), sentence))
        } else {
            Ok((query_str.to_owned(), None, None))
        }
    }

    #[cfg(feature = "tokenizer")]
    async fn format_setence_parts(
        &self,
        parsed: ParseResult<'static, 'a>,
    ) -> Option<Vec<SentencePart>> {
        if parsed.items.len() == 1 {
            return None;
        }

        // Lexemes from `parsed` converted to sentence parts
        let mut sentence_parts = parsed
            .items
            .into_iter()
            .enumerate()
            .map(|(pos, i)| i.into_sentence_part(pos as i32))
            .collect_vec();

        // Request furigana for each kanji containing part
        for part in sentence_parts.iter_mut() {
            if part.text.has_kanji() {
                part.furigana = models::dict::furigana_by_reading(self.pool, &part.text)
                    .await
                    .ok()
                    .and_then(|i| i);
            }
        }

        Some(sentence_parts)
    }

    /// Returns a vec of all PartOfSpeech to filter  
    fn get_pos_filter_from_query(&self) -> Vec<PosSimple> {
        self.query
            .tags
            .iter()
            .filter_map(|i| match i {
                Tag::PartOfSpeech(i) => Some(*i),
                _ => None,
            })
            .collect_vec()
    }

    #[cfg(feature = "tokenizer")]
    fn get_pos_filter(
        &self,
        sentence: &Option<Vec<SentencePart>>,
        morpheme: &Option<WordItem>,
    ) -> Vec<PosSimple> {
        if let Some(ref sentence) = sentence {
            if let Some(ref morpheme) = morpheme {
                if !sentence.is_empty() {
                    if let Some(ref wc) = morpheme.word_class {
                        if let Some(pos) = word_class_to_pos_s(wc) {
                            return vec![pos];
                        }
                    }
                }

                // We don't want to allow tags in sentence reader
                return vec![];
            }
        }

        self.get_pos_filter_from_query()
    }

    #[cfg(not(feature = "tokenizer"))]
    fn get_pos_filter(&self) -> Vec<PosSimple> {
        self.get_pos_filter_from_query()
    }

    /// Perform a native word search
    async fn native_results(&self, query_str: &str) -> Result<ResultData, Error> {
        if self.query.language != QueryLang::Japanese && !query_str.is_japanese() {
            return Ok(ResultData::default());
        }

        #[cfg(feature = "tokenizer")]
        let (query, morpheme, sentence) = self.get_query(query_str).await?;

        #[cfg(not(feature = "tokenizer"))]
        let (query, sentence) = self.get_query(query_str);

        #[cfg(not(feature = "tokenizer"))]
        let morpheme = true;

        #[cfg(feature = "tokenizer")]
        let pos_filter_tags = self.get_pos_filter(&sentence, &morpheme);

        #[cfg(not(feature = "tokenizer"))]
        let pos_filter_tags = self.get_pos_filter();

        let query_modified = query != query_str;

        #[cfg(feature = "tokenizer")]
        let infl_info = morpheme.as_ref().and_then(|i| {
            (!i.inflections.is_empty()).then(|| InflectionInformation {
                lexeme: i.lexeme.to_owned(),
                forms: i.inflections.clone(),
            })
        });

        #[cfg(not(feature = "tokenizer"))]
        let infl_info: Option<InflectionInformation> = None;

        // Define basic search structure
        let mut word_search = WordSearch::new(self.pool, &query);
        word_search
            .with_language(self.query.settings.user_lang)
            .with_english_glosses(self.query.settings.show_english);

        if !pos_filter_tags.is_empty() {
            word_search.with_pos_filter(&pos_filter_tags);
        }

        // Perform the word search
        let (wordresults, original_len) = if real_string_len(&query) == 1 && query.is_kana() {
            // Search for exact matches only if query.len() <= 2

            let res = word_search
                .with_mode(SearchMode::Exact)
                .with_language(self.query.settings.user_lang)
                .search_native(|s| self.ma_f(s, morpheme.clone()))
                .await?;

            let r = if res.0.is_empty() {
                // Do another search if no exact result was found
                word_search
                    .with_mode(SearchMode::RightVariable)
                    .search_native(|s| self.ma_f(s, morpheme.clone()))
                    .await?
            } else {
                res
            };

            if query_modified {
                let origi_res = word_search
                    .with_query(&self.query.query)
                    .with_mode(SearchMode::Exact)
                    .search_native(|s| self.ma_f(s, morpheme.clone()))
                    .await?;

                (
                    r.0.into_iter().chain(origi_res.0).collect(),
                    r.1 + origi_res.1,
                )
            } else {
                r
            }
        } else {
            let results = word_search
                .with_mode(SearchMode::RightVariable)
                .search_native(|s| self.ma_f(s, morpheme.clone()))
                .await?;

            // if query was modified search for the original term too
            let mut results = if query_modified {
                let origi_res = word_search
                    .with_query(&self.query.query)
                    .with_mode(SearchMode::Exact)
                    .search_native(|s| self.ma_f(s, morpheme.clone()))
                    .await?;

                (
                    results.0.into_iter().chain(origi_res.0).collect(),
                    results.1 + origi_res.1,
                )
            } else {
                results
            };

            #[cfg(feature = "tokenizer")]
            let search_order = SearchOrder::new(self.query, &morpheme);

            #[cfg(not(feature = "tokenizer"))]
            let search_order = SearchOrder::new(self.query);

            // Sort the results based
            search_order.sort(&mut results.0, order::native_search_order);
            results.0.dedup();

            results
        };

        #[cfg(feature = "tokenizer")]
        let searched_query = morpheme
            .map(|i| i.original_word.to_owned())
            .unwrap_or(query);

        #[cfg(not(feature = "tokenizer"))]
        let searched_query = query;

        Ok(ResultData {
            words: wordresults,
            infl_info,
            count: original_len,
            sentence_parts: sentence,
            sentence_index: self.query.word_index as i32,
            searched_query,
        })
    }

    /// Search gloss readings
    async fn gloss_results(&self) -> Result<ResultData, Error> {
        if !(self.query.language == QueryLang::Foreign
            || self.query.language == QueryLang::Undetected)
        {
            return Ok(ResultData::default());
        }

        let mode = if real_string_len(&self.query.query) <= 2 {
            SearchMode::Exact
        } else {
            SearchMode::Variable
        };

        let mut word_search = WordSearch::new(self.pool, &self.query.query);
        word_search
            .with_language(self.query.settings.user_lang)
            .with_case_insensitivity(true)
            .with_english_glosses(self.query.settings.show_english)
            .with_mode(mode);

        if self.query.has_part_of_speech_tags() {
            word_search.with_pos_filter(&self.query.get_part_of_speech_tags());
        }

        let mut wordresults = word_search.search_by_glosses().await?;

        // Do romaji search if no results were found
        if wordresults.is_empty() && !self.query.query.is_japanese() {
            return self
                .native_results(&self.query.query.replace(" ", "").to_hiragana())
                .await;
        }

        #[cfg(feature = "tokenizer")]
        let search_order = SearchOrder::new(self.query, &None);

        #[cfg(not(feature = "tokenizer"))]
        let search_order = SearchOrder::new(self.query);

        // Sort the results based
        //GlossWordOrder::new(&self.query.query).sort(&mut wordresults);
        search_order.sort(&mut wordresults, order::foreign_search_order);
        wordresults.dedup();

        let count = wordresults.len();

        // Limit search to 10 results
        wordresults.truncate(10);

        Ok(ResultData {
            words: wordresults,
            count,
            ..Default::default()
        })
    }

    fn merge_words_with_kanji(words: Vec<Word>, kanji: Vec<KanjiResult>) -> Vec<Item> {
        kanji
            .into_iter()
            .map(|i| i.into())
            .collect::<Vec<Item>>()
            .into_iter()
            .chain(words.into_iter().map(|i| i.into()).collect_vec())
            .collect_vec()
    }

    async fn get_cache(&self) -> Option<WordResult> {
        SEARCH_CACHE
            .lock()
            .await
            .cache_get(&self.query.get_hash())
            .cloned()
    }

    async fn save_cache(&self, result: WordResult) {
        SEARCH_CACHE
            .lock()
            .await
            .cache_set(self.query.get_hash(), result);
    }

    #[cfg(feature = "tokenizer")]
    fn ma_f(&self, w: &mut Vec<Word>, morpheme: Option<WordItem>) {
        #[cfg(feature = "tokenizer")]
        let search_order = SearchOrder::new(self.query, &morpheme);

        // Sort the results based
        search_order.sort(w, order::native_search_order);
        w.dedup();

        // Limit search to 10 results
        w.truncate(10);
    }

    #[cfg(not(feature = "tokenizer"))]
    fn ma_f(&self, w: &mut Vec<Word>, _morpheme: bool) {
        #[cfg(not(feature = "tokenizer"))]
        let search_order = SearchOrder::new(self.query);

        // Sort the results based
        search_order.sort(w, order::native_search_order);
        w.dedup();

        // Limit search to 10 results
        w.truncate(10);
    }
}

#[cfg(feature = "tokenizer")]
fn word_class_to_pos_s(class: &WordClass) -> Option<PosSimple> {
    Some(match class {
        WordClass::Particle(_) => PosSimple::Particle,
        WordClass::Verb(_) => PosSimple::Verb,
        WordClass::Adjective(_) => PosSimple::Adjective,
        WordClass::Adverb => PosSimple::Adverb,
        WordClass::Noun(_) => PosSimple::Noun,
        WordClass::Pronoun => PosSimple::Pronoun,
        WordClass::Interjection => PosSimple::Interjection,
        WordClass::Suffix => PosSimple::Suffix,
        WordClass::Prefix => PosSimple::Prefix,
        _ => return None,
    })
}
