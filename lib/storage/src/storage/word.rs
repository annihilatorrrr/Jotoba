use intmap::IntMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use types::jotoba::words::Word;

/// Storage containing all data related to words
#[derive(Serialize, Deserialize, Default)]
pub struct WordStorage {
    /// Word index
    pub words: IntMap<Word>,

    // Search tags
    pub jlpt_word_map: HashMap<u8, Vec<u32>>,
    pub irregular_ichidan: Vec<u32>,
}

impl WordStorage {
    pub fn new() -> Self {
        Self::default()
    }
}
