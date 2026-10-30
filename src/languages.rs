use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};
use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_openstep_plist() -> *const ();
    fn tree_sitter_xcconfig() -> *const ();
    fn tree_sitter_proguard() -> *const ();
    fn tree_sitter_gitignore() -> *const ();
}

const OPENSTEP_PLIST: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_openstep_plist) };
const XCCONFIG: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_xcconfig) };
const PROGUARD: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_proguard) };
const GITIGNORE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_gitignore) };

struct Grammar {
    name: &'static str,
    language: tree_sitter::Language,
    highlights: &'static str,
    injections: &'static str,
    locals: &'static str,
}

fn grammars() -> Vec<Grammar> {
    vec![
        Grammar {
            name: "xml",
            language: tree_sitter_xml::LANGUAGE_XML.into(),
            highlights: tree_sitter_xml::XML_HIGHLIGHT_QUERY,
            injections: "",
            locals: "",
        },
        Grammar {
            name: "groovy",
            language: tree_sitter_groovy::LANGUAGE.into(),
            highlights: include_str!("../assets/queries/groovy-highlights.scm"),
            injections: "",
            locals: "",
        },
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
            name: "batch",
            language: tree_sitter_batch::LANGUAGE.into(),
            highlights: tree_sitter_batch::HIGHLIGHTS_QUERY,
            injections: "",
            locals: "",
        },
        Grammar {
            name: "openstep_plist",
            language: OPENSTEP_PLIST.into(),
            highlights: include_str!("../assets/queries/openstep-plist-highlights.scm"),
            injections: "",
            locals: "",
        },
        Grammar {
            name: "xcconfig",
            language: XCCONFIG.into(),
            highlights: include_str!("../assets/queries/xcconfig-highlights.scm"),
            injections: "",
            locals: "",
        },
        Grammar {
            name: "proguard",
            language: PROGUARD.into(),
            highlights: include_str!("../assets/queries/proguard-highlights.scm"),
            injections: "",
            locals: "",
        },
        Grammar {
            name: "gitignore",
            language: GITIGNORE.into(),
            highlights: include_str!("../assets/queries/gitignore-highlights.scm"),
            injections: "",
            locals: "",
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

    fn parse(name: &str, source: &str) -> tree_sitter::Tree {
        let grammar = grammars().into_iter().find(|g| g.name == name).expect(name);
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&grammar.language).expect("ABI mismatch");
        parser.parse(source, None).expect("parse")
    }

    fn captures(name: &str, source: &str) -> Vec<(String, String)> {
        use tree_sitter::StreamingIterator;
        let grammar = grammars().into_iter().find(|g| g.name == name).expect(name);
        let tree = parse(name, source);
        let query = tree_sitter::Query::new(&grammar.language, grammar.highlights).unwrap();
        let mut cursor = tree_sitter::QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
        let mut out = Vec::new();
        while let Some(m) = matches.next() {
            for cap in m.captures {
                let text = cap.node.utf8_text(source.as_bytes()).unwrap().to_string();
                out.push((text, query.capture_names()[cap.index as usize].to_string()));
            }
        }
        out
    }

    fn has(captures: &[(String, String)], text: &str, capture: &str) -> bool {
        captures.iter().any(|(t, c)| t == text && c == capture)
    }

    #[test]
    fn in_repo_grammars_parse_real_world_samples() {
        let samples = [
            (
                "openstep_plist",
                "// !$*UTF8*$!\n{\n\tarchiveVersion = 1;\n\tobjects = {\n/* Begin PBXBuildFile section */\n\t\t62431E952F9E73BC00C35025 /* Foo in Frameworks */ = {isa = PBXBuildFile; productRef = 62431E942F9E73BC00C35025 /* Foo */; };\n\t\tAB = {files = (\n\t\t\tA /* a */,\n\t\t\tB,\n\t\t); path = Foo.swift; LD = \"@executable_path/Frameworks\"; ON = YES; };\n\t};\n}\n",
            ),
            ("openstep_plist", "/* Title */\n\"hello\" = \"Hi \\\"there\\\"\";\n// note\n\"bye\" = \"Bye\";\n"),
            (
                "xcconfig",
                "// Keys\n#include? \"Local.xcconfig\"\nAPI_KEY = abc123\nURL = https:/$()/example.com/$(PATH) // trailing\nKEY[config=Release][sdk=iphoneos*] = \"x\" ${Y}\nEMPTY =\n",
            ),
            (
                "proguard",
                "# Keep\n-keepattributes *Annotation*, Signature\n-keep public class * extends android.app.Activity\n-keepclassmembers class kotlinx.coroutines.** {\n    volatile <fields>;\n    public static **[] values();\n}\n-keep @androidx.annotation.Keep class * { *; }\n-optimizationpasses 5\n-printmapping build/mapping.txt\n",
            ),
            ("gitignore", "# Build\n*.iml\n/build\n!keep/**/x?.txt\n[Dd]erived\\ Data/\n.DS_Store\n"),
            ("batch", "@rem Gradle startup\n@if \"%DEBUG%\"==\"\" @echo off\nset DIRNAME=%~dp0\nif \"%ERRORLEVEL%\"==\"0\" goto execute\n:execute\n\"%JAVA_EXE%\" %* \n"),
        ];
        for (name, source) in samples {
            let tree = parse(name, source);
            assert!(!tree.root_node().has_error(), "{name}: {}", tree.root_node().to_sexp());
        }
    }

    #[test]
    fn in_repo_queries_use_styled_capture_names() {
        let pbx = captures("openstep_plist", "{ isa = PBXBuildFile; ON = YES; n = 77; p = \"a b\"; /* c */ }");
        assert!(has(&pbx, "isa", "property"));
        assert!(has(&pbx, "PBXBuildFile", "type"));
        assert!(has(&pbx, "YES", "boolean"));
        assert!(has(&pbx, "77", "number"));
        assert!(has(&pbx, "\"a b\"", "string"));
        assert!(has(&pbx, "/* c */", "comment"));

        let xc = captures("xcconfig", "A[sdk=*] = x/$(B) // c\n");
        assert!(has(&xc, "A", "property"));
        assert!(has(&xc, "sdk", "attribute"));
        assert!(has(&xc, "$(B)", "variable.special"));
        assert!(has(&xc, "// c", "comment"), "{xc:?}");
        assert!(!xc.iter().any(|(t, _)| t.contains("//") && t != "// c"));

        let pro = captures("proguard", "-keep class a.B { *; } # c\n");
        assert!(has(&pro, "-keep", "keyword"));
        assert!(has(&pro, "class", "keyword"));
        assert!(has(&pro, "a.B", "type"));
        assert!(has(&pro, "# c", "comment"));
    }
}
