//! Language pack: grammars on top of the ones gpui-component ships.
//!
//! Each entry is a tree-sitter grammar crate plus its highlight queries. To add
//! a language: add the crate to Cargo.toml, add a line here, and map its file
//! extensions in `files::language_for`.

use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};

struct Grammar {
    name: &'static str,
    language: tree_sitter::Language,
    highlights: &'static str,
    injections: &'static str,
    locals: &'static str,
}

fn grammars() -> Vec<Grammar> {
    vec![
        // Android manifests, layouts, resources, plists.
        Grammar {
            name: "xml",
            language: tree_sitter_xml::LANGUAGE_XML.into(),
            highlights: tree_sitter_xml::XML_HIGHLIGHT_QUERY,
            injections: "",
            locals: "",
        },
        // build.gradle, Jenkinsfile.
        Grammar {
            name: "groovy",
            language: tree_sitter_groovy::LANGUAGE.into(),
            highlights: include_str!("../assets/queries/groovy-highlights.scm"),
            injections: "",
            locals: "",
        },
        // gradle.properties, local.properties.
        Grammar {
            name: "properties",
            language: tree_sitter_properties::LANGUAGE.into(),
            highlights: tree_sitter_properties::HIGHLIGHTS_QUERY,
            injections: "",
            locals: "",
        },
        Grammar {
            name: "objc",
            language: tree_sitter_objc::LANGUAGE.into(),
            highlights: tree_sitter_objc::HIGHLIGHTS_QUERY,
            injections: "",
            locals: tree_sitter_objc::LOCALS_QUERY,
        },
        Grammar {
            name: "ini",
            language: tree_sitter_ini::LANGUAGE.into(),
            highlights: tree_sitter_ini::HIGHLIGHTS_QUERY,
            injections: "",
            locals: "",
        },
        Grammar {
            name: "dart",
            language: tree_sitter_dart::LANGUAGE.into(),
            highlights: tree_sitter_dart::HIGHLIGHTS_QUERY,
            injections: "",
            locals: tree_sitter_dart::LOCALS_QUERY,
        },
    ]
}

pub fn register() {
    let registry = LanguageRegistry::singleton();
    for grammar in grammars() {
        let config = GrammarConfig::new(
            grammar.name,
            grammar.language,
            vec![],
            grammar.highlights,
            grammar.injections,
            grammar.locals,
        );
        registry.register(grammar.name, &config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_highlight_query_compiles() {
        for grammar in grammars() {
            if let Err(err) = tree_sitter::Query::new(&grammar.language, grammar.highlights) {
                panic!("{} highlights query is invalid: {err}", grammar.name);
            }
        }
    }
}
