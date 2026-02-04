// SPDX-License-Identifier: MIT OR Apache-2.0

//! Language registry for tree-sitter parsers

use once_cell::sync::Lazy;
use std::collections::HashMap;
use tree_sitter::Language;

/// Supported languages with their tree-sitter parsers
pub struct LanguageRegistry {
    languages: HashMap<String, Language>,
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageRegistry {
    pub fn new() -> Self {
        let mut languages = HashMap::new();

        // Register supported languages
        languages.insert(
            "typescript".into(),
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        );
        languages.insert("tsx".into(), tree_sitter_typescript::LANGUAGE_TSX.into());
        languages.insert("javascript".into(), tree_sitter_javascript::LANGUAGE.into());
        languages.insert("python".into(), tree_sitter_python::LANGUAGE.into());
        languages.insert("rust".into(), tree_sitter_rust::LANGUAGE.into());
        languages.insert("go".into(), tree_sitter_go::LANGUAGE.into());
        languages.insert("c".into(), tree_sitter_c::LANGUAGE.into());
        languages.insert("cpp".into(), tree_sitter_cpp::LANGUAGE.into());
        languages.insert("java".into(), tree_sitter_java::LANGUAGE.into());
        languages.insert("ruby".into(), tree_sitter_ruby::LANGUAGE.into());

        Self { languages }
    }

    /// Get language by name
    pub fn get(&self, name: &str) -> Option<&Language> {
        self.languages.get(&name.to_lowercase())
    }

    /// Get parser for language
    #[allow(dead_code)]
    pub fn parser(&self, name: &str) -> Option<tree_sitter::Parser> {
        self.get(name).map(|lang| {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(lang).ok();
            parser
        })
    }

    /// List all supported languages
    #[allow(dead_code)]
    pub fn supported_languages(&self) -> Vec<&str> {
        self.languages.keys().map(|s| s.as_str()).collect()
    }
}

/// Global language registry
pub static LANGUAGES: Lazy<LanguageRegistry> = Lazy::new(LanguageRegistry::new);
