// SPDX-License-Identifier: MIT OR Apache-2.0

//! Symbol extraction from AST using tree-sitter node traversal

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

use crate::parser::languages::LANGUAGES;

/// Symbol kinds
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    Type,
    Variable,
    Constant,
    Enum,
    Module,
    Struct,
    Trait,
    Method,
    Property,
    Unknown,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::Function => write!(f, "function"),
            SymbolKind::Class => write!(f, "class"),
            SymbolKind::Interface => write!(f, "interface"),
            SymbolKind::Type => write!(f, "type"),
            SymbolKind::Variable => write!(f, "variable"),
            SymbolKind::Constant => write!(f, "constant"),
            SymbolKind::Enum => write!(f, "enum"),
            SymbolKind::Module => write!(f, "module"),
            SymbolKind::Struct => write!(f, "struct"),
            SymbolKind::Trait => write!(f, "trait"),
            SymbolKind::Method => write!(f, "method"),
            SymbolKind::Property => write!(f, "property"),
            SymbolKind::Unknown => write!(f, "unknown"),
        }
    }
}

/// Extracted symbol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub scope: Option<String>,
}

/// Symbol extractor using tree-sitter node traversal
pub struct SymbolExtractor;

impl Default for SymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Extract symbols from source code
    pub fn extract(&self, source: &str, language: &str) -> Result<Vec<Symbol>> {
        let lang = LANGUAGES
            .get(language)
            .ok_or_else(|| anyhow::anyhow!("Unsupported language: {}", language))?;

        let mut parser = Parser::new();
        parser.set_language(lang)?;

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse source"))?;

        let source_bytes = source.as_bytes();
        let mut symbols = Vec::new();

        self.traverse_node(tree.root_node(), source_bytes, language, &mut symbols);

        Ok(symbols)
    }

    /// Traverse the AST and extract symbols
    fn traverse_node(&self, node: Node, source: &[u8], lang: &str, symbols: &mut Vec<Symbol>) {
        // Extract symbol based on node type and language
        if let Some(symbol) = self.extract_symbol_from_node(node, source, lang) {
            symbols.push(symbol);
        }

        // Recursively traverse children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.traverse_node(child, source, lang, symbols);
        }
    }

    /// Extract a symbol from a node if it represents a definition
    fn extract_symbol_from_node(&self, node: Node, source: &[u8], lang: &str) -> Option<Symbol> {
        let kind = node.kind();

        // Match patterns based on language
        let (symbol_kind, name_field) = match lang {
            "typescript" | "javascript" => self.match_typescript_node(kind),
            "python" => self.match_python_node(kind),
            "rust" => self.match_rust_node(kind),
            "go" => self.match_go_node(kind),
            "c" => self.match_c_node(kind),
            "cpp" => self.match_cpp_node(kind),
            "java" => self.match_java_node(kind),
            "ruby" => self.match_ruby_node(kind),
            _ => return None,
        }?;

        // Get the name from the appropriate child node
        let name_node = node.child_by_field_name(name_field)?;
        let name = name_node.utf8_text(source).ok()?.to_string();

        Some(Symbol {
            name,
            kind: symbol_kind,
            line: node.start_position().row + 1,
            column: node.start_position().column + 1,
            end_line: node.end_position().row + 1,
            scope: None,
        })
    }

    /// Match TypeScript/JavaScript AST nodes
    fn match_typescript_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "function_declaration" => Some((SymbolKind::Function, "name")),
            "class_declaration" => Some((SymbolKind::Class, "name")),
            "interface_declaration" => Some((SymbolKind::Interface, "name")),
            "type_alias_declaration" => Some((SymbolKind::Type, "name")),
            "enum_declaration" => Some((SymbolKind::Enum, "name")),
            "method_definition" => Some((SymbolKind::Method, "name")),
            "variable_declarator" => Some((SymbolKind::Variable, "name")),
            _ => None,
        }
    }

    /// Match Python AST nodes
    fn match_python_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "function_definition" => Some((SymbolKind::Function, "name")),
            "class_definition" => Some((SymbolKind::Class, "name")),
            _ => None,
        }
    }

    /// Match Rust AST nodes
    fn match_rust_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "function_item" => Some((SymbolKind::Function, "name")),
            "struct_item" => Some((SymbolKind::Struct, "name")),
            "enum_item" => Some((SymbolKind::Enum, "name")),
            "trait_item" => Some((SymbolKind::Trait, "name")),
            "type_item" => Some((SymbolKind::Type, "name")),
            "const_item" => Some((SymbolKind::Constant, "name")),
            "static_item" => Some((SymbolKind::Variable, "name")),
            "mod_item" => Some((SymbolKind::Module, "name")),
            _ => None,
        }
    }

    /// Match Go AST nodes
    fn match_go_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "function_declaration" => Some((SymbolKind::Function, "name")),
            "method_declaration" => Some((SymbolKind::Method, "name")),
            "type_spec" => Some((SymbolKind::Type, "name")),
            _ => None,
        }
    }

    /// Match C AST nodes
    fn match_c_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "function_definition" => Some((SymbolKind::Function, "declarator")),
            "function_declarator" => Some((SymbolKind::Function, "declarator")),
            "struct_specifier" => Some((SymbolKind::Struct, "name")),
            "enum_specifier" => Some((SymbolKind::Enum, "name")),
            "type_definition" => Some((SymbolKind::Type, "declarator")),
            _ => None,
        }
    }

    /// Match C++ AST nodes
    fn match_cpp_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "function_definition" => Some((SymbolKind::Function, "declarator")),
            "function_declarator" => Some((SymbolKind::Function, "declarator")),
            "class_specifier" => Some((SymbolKind::Class, "name")),
            "struct_specifier" => Some((SymbolKind::Struct, "name")),
            "enum_specifier" => Some((SymbolKind::Enum, "name")),
            "namespace_definition" => Some((SymbolKind::Module, "name")),
            "type_definition" => Some((SymbolKind::Type, "declarator")),
            _ => None,
        }
    }

    /// Match Java AST nodes
    fn match_java_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "method_declaration" => Some((SymbolKind::Method, "name")),
            "class_declaration" => Some((SymbolKind::Class, "name")),
            "interface_declaration" => Some((SymbolKind::Interface, "name")),
            "enum_declaration" => Some((SymbolKind::Enum, "name")),
            "constructor_declaration" => Some((SymbolKind::Function, "name")),
            "field_declaration" => Some((SymbolKind::Property, "declarator")),
            _ => None,
        }
    }

    /// Match Ruby AST nodes
    fn match_ruby_node(&self, kind: &str) -> Option<(SymbolKind, &'static str)> {
        match kind {
            "method" => Some((SymbolKind::Method, "name")),
            "singleton_method" => Some((SymbolKind::Method, "name")),
            "class" => Some((SymbolKind::Class, "name")),
            "module" => Some((SymbolKind::Module, "name")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_typescript_function() {
        let source = r#"
function greet(name: string): string {
    return `Hello, ${name}!`;
}
"#;
        let extractor = SymbolExtractor::new();
        let symbols = extractor.extract(source, "typescript").unwrap();

        assert!(!symbols.is_empty());
        let func = symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(func.kind, SymbolKind::Function);
    }

    #[test]
    fn test_extract_typescript_class() {
        let source = r#"
class Person {
    constructor(public name: string) {}
    
    greet(): string {
        return `Hello, ${this.name}!`;
    }
}
"#;
        let extractor = SymbolExtractor::new();
        let symbols = extractor.extract(source, "typescript").unwrap();

        let class = symbols.iter().find(|s| s.name == "Person").unwrap();
        assert_eq!(class.kind, SymbolKind::Class);
    }

    #[test]
    fn test_extract_rust_function() {
        let source = r#"
fn main() {
    println!("Hello, world!");
}

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
        let extractor = SymbolExtractor::new();
        let symbols = extractor.extract(source, "rust").unwrap();

        assert!(symbols
            .iter()
            .any(|s| s.name == "main" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "add" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_extract_python_class() {
        let source = r#"
class Calculator:
    def add(self, a, b):
        return a + b
    
    def subtract(self, a, b):
        return a - b
"#;
        let extractor = SymbolExtractor::new();
        let symbols = extractor.extract(source, "python").unwrap();

        let class = symbols.iter().find(|s| s.name == "Calculator").unwrap();
        assert_eq!(class.kind, SymbolKind::Class);
    }

    #[test]
    fn test_symbol_kind_display() {
        assert_eq!(SymbolKind::Function.to_string(), "function");
        assert_eq!(SymbolKind::Class.to_string(), "class");
        assert_eq!(SymbolKind::Variable.to_string(), "variable");
    }

    #[test]
    fn test_unsupported_language() {
        let extractor = SymbolExtractor::new();
        let result = extractor.extract("code", "unknown_lang");
        assert!(result.is_err());
    }
}
