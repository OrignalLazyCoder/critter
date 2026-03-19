use std::collections::HashMap;

use super::emotions_data;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmotionColor {
    Green,
    Amber,
    Coral,
    Sky,
    Violet,
    Rose,
    Teal,
    Neutral,
}

#[derive(Debug, Clone)]
pub struct EmotionSpec {
    pub key: String,
    pub name: String,
    pub color: EmotionColor,
    pub ms: u64,
    pub frames: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EmotionCatalog {
    specs: HashMap<String, EmotionSpec>,
}

impl EmotionCatalog {
    pub fn load_default() -> Self {
        let mut specs = HashMap::<String, EmotionSpec>::new();
        for spec in emotions_data::all_emotions() {
            specs.insert(spec.key.clone(), spec);
        }
        Self { specs }
    }

    pub fn get(&self, key: &str) -> Option<&EmotionSpec> {
        self.specs.get(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.specs.contains_key(key)
    }
}
