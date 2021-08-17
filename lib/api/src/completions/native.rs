use std::iter::FromIterator;

use futures::{stream::FuturesOrdered, TryStreamExt};
use itertools::Itertools;
use models::queryable::prepared_query;

use super::*;

const MAX_RESULTS: i64 = 10;

/// Get suggestions for foreign search input
pub(super) async fn suggestions(query_str: &str) -> Result<Vec<WordPair>, RestError> {
    /*
    get_sequence_ids(client, &query_str).await
    */
    Ok(vec![])
}

/*
async fn get_sequence_ids(client: &Pool, query_str: &str) -> Result<Vec<WordPair>, RestError> {
    let seq_query = "SELECT sequence FROM dict WHERE reading LIKE $1 ORDER BY jlpt_lvl DESC NULLS LAST, ARRAY_LENGTH(priorities,1) DESC NULLS LAST, LENGTH(reading) LIMIT $2";

    let mut sequences: Vec<i32> = prepared_query(
        &client,
        seq_query,
        &[&format!("{}%", query_str).as_str(), &MAX_RESULTS],
    )
    .await?;

    sequences.dedup();

    Ok(load_words(&client, &sequences).await?)
}

async fn load_words(client: &Pool, sequences: &[i32]) -> Result<Vec<WordPair>, RestError> {
    let word_query =
        "select reading, kanji from dict where sequence = $1 and (is_main or kanji = false)";

    let client = client.get().await?;

    let prepared = client.prepare_cached(word_query).await?;

    let client = &client;
    Ok(FuturesOrdered::from_iter(sequences.into_iter().map(|i| {
        let cloned = prepared.clone();
        async move { client.query(&cloned, &[&i]).await }
    }))
    .try_collect::<Vec<_>>()
    .await?
    .into_iter()
    .filter_map(|word| {
        let words: Vec<(String, bool)> =
            word.into_iter().map(|i| (i.get(0), i.get(1))).collect_vec();

        let kana = words.iter().find(|i| !i.1)?.0.to_owned();
        let kanji = words.iter().find(|i| i.1).map(|i| i.0.to_owned());

        Some(WordPair {
            primary: kana,
            secondary: kanji,
        })
    })
    .collect())
}
*/
