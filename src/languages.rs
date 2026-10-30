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
            highlights: include_str!("../assets/queries/objc-highlights.scm"),
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

const OVERRIDES: &[(&str, &str)] = &[
    ("swift", include_str!("../assets/queries/swift-highlights.scm")),
    ("kotlin", include_str!("../assets/queries/kotlin-highlights.scm")),
    ("c", include_str!("../assets/queries/c-highlights.scm")),
    ("cpp", include_str!("../assets/queries/cpp-highlights.scm")),
    ("go", include_str!("../assets/queries/go-highlights.scm")),
    ("java", include_str!("../assets/queries/java-highlights.scm")),
    ("javascript", include_str!("../assets/queries/javascript-highlights.scm")),
    ("typescript", include_str!("../assets/queries/typescript-highlights.scm")),
    ("tsx", include_str!("../assets/queries/tsx-highlights.scm")),
];

pub fn register() {
    let registry = LanguageRegistry::singleton();
    for (name, highlights) in OVERRIDES {
        if let Some(mut config) = registry.language(name) {
            config.highlights = (*highlights).into();
            registry.register(name, &config);
        }
    }
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
    fn override_queries_compile_and_use_themed_captures() {
        let registry = LanguageRegistry::singleton();
        let theme = crate::theme::highlight_theme(gpui_kit::component::ThemeMode::Dark);
        let mut unthemed = Vec::new();
        for (name, highlights) in OVERRIDES {
            let language = registry.language(name).expect(name).language.expect(name);
            let query = match tree_sitter::Query::new(&language, highlights) {
                Ok(query) => query,
                Err(err) => panic!("{name} override query is invalid: {err}"),
            };
            for capture in query.capture_names() {
                if !capture.starts_with('_') && theme.style.syntax.style(capture).is_none() {
                    unthemed.push(format!("{name}: @{capture}"));
                }
            }
        }
        assert!(unthemed.is_empty(), "{unthemed:#?}");
    }

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

    fn colors(language: &str, source: &str) -> Vec<(String, u32)> {
        use gpui_kit::component::highlighter::SyntaxHighlighter;
        use gpui_kit::component::input::Rope;
        register();
        let theme = crate::theme::highlight_theme(gpui_kit::component::ThemeMode::Dark);
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &Rope::from(source), None);
        highlighter
            .styles(&(0..source.len()), &*theme)
            .into_iter()
            .filter_map(|(range, style)| {
                let color = style.color?;
                let rgba: gpui_kit::Rgba = color.into();
                let packed = ((rgba.r * 255.).round() as u32) << 16 | ((rgba.g * 255.).round() as u32) << 8 | (rgba.b * 255.).round() as u32;
                Some((source[range].to_string(), packed))
            })
            .collect()
    }

    fn color_of(colors: &[(String, u32)], text: &str) -> Option<u32> {
        colors.iter().rev().find(|(t, _)| t == text).map(|(_, c)| *c)
    }

    const KEYWORD: u32 = 0xfc5fa3;
    const STRING: u32 = 0xfc6a5d;
    const COMMENT: u32 = 0x6c7986;
    const NUMBER: u32 = 0xd0bf69;
    const TYPE: u32 = 0x9ef1dd;
    const SYSTEM_TYPE: u32 = 0xd0a8ff;
    const FUNCTION: u32 = 0x67b7a4;
    const ATTRIBUTE: u32 = 0xbf8555;
    const PREPROC: u32 = 0xfd8f3f;

    #[test]
    fn swift_uses_xcode_colors() {
        let src = "import Foundation\n@MainActor\nfinal class Foo: Bar {\n    // note\n    let n = 42\n    func run(_ x: Int) -> String { print(\"a \\(x)\"); return \"b\" }\n}\n";
        let c = colors("swift", src);
        assert_eq!(color_of(&c, "import"), Some(KEYWORD));
        assert_eq!(color_of(&c, "class"), Some(KEYWORD));
        assert_eq!(color_of(&c, "func"), Some(KEYWORD));
        assert_eq!(color_of(&c, "return"), Some(KEYWORD));
        assert_eq!(color_of(&c, "42"), Some(NUMBER));
        assert_eq!(color_of(&c, "// note"), Some(COMMENT));
        assert_eq!(color_of(&c, "Bar"), Some(TYPE));
        assert_eq!(color_of(&c, "MainActor"), Some(ATTRIBUTE));
        assert_eq!(color_of(&c, "run"), Some(FUNCTION));
        assert_eq!(color_of(&c, "print"), Some(FUNCTION));
        assert_eq!(color_of(&c, "b"), Some(STRING));
    }

    #[test]
    fn kotlin_and_gradle_dsl_highlight() {
        let src = "plugins { id(\"com.android.application\") }\n@Composable\nfun greet(name: String): Int { val x = 1; return \"hi $name\".length }\n";
        let c = colors("kotlin", src);
        assert_eq!(color_of(&c, "fun"), Some(KEYWORD));
        assert_eq!(color_of(&c, "val"), Some(KEYWORD));
        assert_eq!(color_of(&c, "1"), Some(NUMBER));
        assert_eq!(color_of(&c, "Composable"), Some(ATTRIBUTE));
        assert_eq!(color_of(&c, "greet"), Some(FUNCTION));
        assert_eq!(color_of(&c, "String"), Some(TYPE));
    }

    #[test]
    fn groovy_gradle_highlights_calls_not_bodies() {
        let src = "dependencies {\n    implementation 'x:y:1'\n    def n = 3\n}\n";
        let c = colors("groovy", src);
        assert_eq!(color_of(&c, "implementation"), Some(FUNCTION));
        assert_eq!(color_of(&c, "def"), Some(KEYWORD));
        assert!(color_of(&c, "dependencies").is_some());
    }

    #[test]
    fn cpp_and_c_highlight_functions_types_and_preprocessor() {
        let src = "#include <vector>\n#define MAX 3\nclass A { public: int f(int x) const; };\nint main() { std::string s = \"a\"; return f(1); }\n";
        let c = colors("cpp", src);
        assert_eq!(color_of(&c, "class"), Some(KEYWORD));
        assert_eq!(color_of(&c, "return"), Some(KEYWORD));
        assert_eq!(color_of(&c, "\"a\""), Some(STRING));
        assert_eq!(color_of(&c, "main"), Some(FUNCTION));
        assert_eq!(color_of(&c, "int"), Some(SYSTEM_TYPE));
        assert_eq!(color_of(&c, "#define"), Some(PREPROC));
        let c = colors("c", "#include <stdio.h>\nstatic int add(int a) { return a + 1; }\n");
        assert_eq!(color_of(&c, "add"), Some(FUNCTION));
        assert_eq!(color_of(&c, "static"), Some(KEYWORD));
    }

    #[test]
    fn go_js_ts_tsx_highlight() {
        let c = colors("go", "package main\nfunc add(a int) int { return a + 1 }\n");
        assert_eq!(color_of(&c, "func"), Some(KEYWORD));
        assert_eq!(color_of(&c, "add"), Some(FUNCTION));
        let c = colors("javascript", "const f = (a) => { return a + 1 }\nf(2, 'x')\n");
        assert_eq!(color_of(&c, "const"), Some(KEYWORD));
        assert_eq!(color_of(&c, "f"), Some(FUNCTION));
        assert_eq!(color_of(&c, "'x'"), Some(STRING));
        let c = colors("typescript", "interface P { a: number }\nfunction g(p: P): string { return \"s\" }\n");
        assert_eq!(color_of(&c, "interface"), Some(KEYWORD));
        assert_eq!(color_of(&c, "g"), Some(FUNCTION));
        assert_eq!(color_of(&c, "number"), Some(SYSTEM_TYPE));
        let c = colors("tsx", "export const A = (p: Props) => <div className=\"x\">{p.a}</div>;\n");
        assert_eq!(color_of(&c, "export"), Some(KEYWORD));
        assert_eq!(color_of(&c, "\"x\""), Some(STRING));
        assert!(color_of(&c, "div").is_some());
    }

    #[test]
    fn swiftui_source_and_interface_files_highlight() {
        let src = "import SwiftUI\n\nstruct ContentView: View {\n    @State private var count = 0\n    @Environment(\\.dismiss) var dismiss\n    var body: some View {\n        VStack(spacing: 8) {\n            Text(\"Count \\(count)\")\n                .font(.title)\n            Button(\"Add\") { count += 1 }\n        }\n        .padding()\n    }\n}\n\n#Preview { ContentView() }\n";
        let c = colors("swift", src);
        assert_eq!(color_of(&c, "struct"), Some(KEYWORD));
        assert_eq!(color_of(&c, "private"), Some(KEYWORD));
        assert_eq!(color_of(&c, "some"), Some(KEYWORD));
        assert_eq!(color_of(&c, "var"), Some(KEYWORD));
        assert_eq!(color_of(&c, "State"), Some(ATTRIBUTE));
                assert!(color_of(&c, "View").is_some());
        assert!(color_of(&c, "title").is_some());
        let interface = "// swift-interface-format-version: 1.0\n// swift-compiler-version: Apple Swift version 6.0\nimport Swift\n@available(macOS 10.10, *)\n@frozen public struct Date : Swift.Hashable {\n  public init(timeIntervalSinceNow: Foundation.TimeInterval)\n  public var timeIntervalSince1970: Foundation.TimeInterval { get }\n}\n";
        let path = std::path::Path::new("/tmp/sourcekit-lsp/GeneratedInterfaces/1/Foundation.swiftinterface");
        assert_eq!(crate::files::detect_language(path, interface), "swift");
        let c = colors("swift", interface);
        assert_eq!(color_of(&c, "import"), Some(KEYWORD));
        assert_eq!(color_of(&c, "public"), Some(KEYWORD));
        assert_eq!(color_of(&c, "struct"), Some(KEYWORD));
        assert_eq!(color_of(&c, "var"), Some(KEYWORD));
        assert_eq!(color_of(&c, "available"), Some(ATTRIBUTE));
        assert!(color_of(&c, "// swift-interface-format-version: 1.0").is_some());
    }

    #[test]
    #[ignore]
    fn swift_highlights_real_files_and_times_big_interface() {
        use gpui_kit::component::highlighter::SyntaxHighlighter;
        use gpui_kit::component::input::Rope;
        register();
        let theme = crate::theme::highlight_theme(gpui_kit::component::ThemeMode::Dark);
        let home = std::env::var("HOME").unwrap();
        let real = format!("{home}/Desktop/Projects/ToRelease/Remote-App/DmoteIOS/DMote/App/ContentView.swift");
        let mut paths = vec![real];
        if let Ok(found) = std::process::Command::new("sh")
            .arg("-c")
            .arg("find /var/folders -name Foundation.swiftinterface 2>/dev/null | head -1")
            .output()
        {
            let line = String::from_utf8_lossy(&found.stdout).trim().to_string();
            if !line.is_empty() {
                paths.push(line);
            }
        }
        for path in paths {
            let source = std::fs::read_to_string(&path).unwrap();
            assert_eq!(crate::files::detect_language(std::path::Path::new(&path), &source), "swift");
            let start = std::time::Instant::now();
            let mut highlighter = SyntaxHighlighter::new("swift");
            highlighter.update(None, &Rope::from(source.as_str()), None);
            let parse = start.elapsed();
            let styled = highlighter.styles(&(0..source.len()), &*theme);
            let colored = styled.iter().filter(|(_, s)| s.color.is_some()).count();
            eprintln!("{path}: {} lines, parse+update {parse:?}, total {:?}, {colored} colored spans", source.lines().count(), start.elapsed());
            assert!(colored > source.lines().count() / 4, "{path}: only {colored} colored spans");
        }
    }
}
