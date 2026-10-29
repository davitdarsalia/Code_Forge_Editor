//! Full-color file-type icons for the tree, tabs, ⌘P and search results.
//!
//! GPUI's `svg()` element rasterizes an alpha mask and tints it with the text
//! color, so multi-color SVGs lose their colors. `img()` instead decodes the
//! SVG through resvg into an RGBA image (cached by the asset system after the
//! first load), so the icons keep their own colors. The SVGs are embedded
//! with `include_bytes!` and served by the app's `Assets` source.
//! Regenerate the artwork with `scripts/gen_file_icons.py`.

use std::path::Path;

use gpui_kit::{Img, Styled as _, img};

/// Icon edge in points at 100% (Settings ▸ Icon size scales it). The SVGs
/// declare 32x32 so GPUI's 2x raster is 64 px: every scale up to 200% is
/// drawn by downsampling, which stays sharp.
pub const ICON_SIZE: f32 = 14.;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        /// (asset path, bytes) for every bundled file icon.
        pub const ASSETS: &[(&str, &[u8])] = &[
            $((concat!("file-icons/", $name, ".svg"), include_bytes!(concat!("../assets/file-icons/", $name, ".svg")))),*
        ];
    };
}

icons!(
    "c", "cpp", "css", "docker", "env", "file", "git", "go", "gradle", "header", "html", "image", "java",
    "javascript", "json", "kotlin", "license", "lock", "markdown", "plist", "python", "react", "readme", "ruby",
    "rust", "shell", "swift", "text", "toml", "typescript", "xcodeproj", "xcworkspace", "xml", "yaml",
    "folder", "folder-open",
    "folder-src", "folder-src-open", "folder-assets", "folder-assets-open", "folder-test", "folder-test-open",
    "folder-github", "folder-github-open", "folder-build", "folder-build-open", "folder-deps", "folder-deps-open",
    "folder-ios", "folder-ios-open", "folder-android", "folder-android-open", "folder-gradle", "folder-gradle-open",
);

/// A sized, full-color icon element for a file path.
pub fn file_icon_img(path: &Path) -> Img {
    sized(file_icon(path))
}

/// A sized, full-color icon element for a directory.
pub fn folder_icon_img(path: &Path, expanded: bool) -> Img {
    sized(folder_icon(path, expanded))
}

fn sized(asset: &'static str) -> Img {
    img(asset).size(crate::icons::scaled(ICON_SIZE)).flex_none()
}

/// Asset path of the icon for a file. Exact names win over extensions.
pub fn file_icon(path: &Path) -> &'static str {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    asset(file_kind(&name))
}

/// Asset path of the icon for a directory, open or closed.
pub fn folder_icon(path: &Path, expanded: bool) -> &'static str {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    // Bundles that look like folders on disk but are documents in Xcode.
    if name.ends_with(".xcodeproj") {
        return asset("xcodeproj");
    }
    if name.ends_with(".xcworkspace") {
        return asset("xcworkspace");
    }
    match (folder_kind(&name), expanded) {
        (None, false) => asset("folder"),
        (None, true) => asset("folder-open"),
        (Some(kind), false) => asset(&format!("folder-{kind}")),
        (Some(kind), true) => asset(&format!("folder-{kind}-open")),
    }
}

fn asset(stem: &str) -> &'static str {
    ASSETS
        .iter()
        .map(|(p, _)| *p)
        .find(|p| p.strip_prefix("file-icons/").and_then(|p| p.strip_suffix(".svg")) == Some(stem))
        .unwrap_or("file-icons/file.svg")
}

/// Icon stem for a lowercase file name.
fn file_kind(name: &str) -> &'static str {
    match name {
        "dockerfile" | "containerfile" | ".dockerignore" => return "docker",
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" | ".mailmap" => return "git",
        "gemfile" | "rakefile" | "podfile" | "fastfile" | "appfile" | "matchfile" | "brewfile" | "dangerfile" => {
            return "ruby";
        }
        "gradlew" | "gradlew.bat" | ".zshrc" | ".bashrc" | ".profile" | ".zprofile" | "makefile" => return "shell",
        "gradle.properties" | "gradle-wrapper.properties" | "gradle.lockfile" => return "gradle",
        "package-lock.json" | "package.resolved" | "pnpm-lock.yaml" | "podfile.lock" | "gemfile.lock" => {
            return "lock";
        }
        "project.pbxproj" => return "xcodeproj",
        _ => {}
    }
    if name.starts_with("readme") {
        return "readme";
    }
    if name.starts_with("license") || name.starts_with("licence") || name.starts_with("copying") {
        return "license";
    }
    if name == ".env" || name.starts_with(".env.") || name.ends_with(".env") {
        return "env";
    }
    if name.starts_with("build.gradle") || name.starts_with("settings.gradle") {
        return "gradle";
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "java" | "class" | "jar" => "java",
        "gradle" => "gradle",
        "xml" | "xsd" | "xib" | "storyboard" | "svgz" => "xml",
        "json" | "jsonc" | "json5" | "webmanifest" | "geojson" => "json",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" | "mdx" => "markdown",
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" | "jsx" => "react",
        "py" | "pyi" | "pyw" => "python",
        "rb" | "gemspec" | "rake" | "erb" => "ruby",
        "go" | "mod" | "sum" => "go",
        "c" | "m" => "c",
        "cpp" | "cc" | "cxx" | "c++" | "mm" | "hpp" | "hh" | "hxx" => "cpp",
        "h" => "header",
        "css" | "scss" | "sass" | "less" => "css",
        "html" | "htm" | "xhtml" => "html",
        "sh" | "bash" | "zsh" | "fish" | "command" | "bat" | "ps1" => "shell",
        "plist" | "entitlements" | "xcconfig" | "strings" | "stringsdict" | "xcprivacy" => "plist",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "icns" | "bmp" | "tif" | "tiff" | "heic"
        | "avif" => "image",
        "lock" | "lockfile" | "resolved" => "lock",
        "txt" | "log" | "rtf" | "csv" | "tsv" | "properties" | "ini" | "cfg" | "conf" => "text",
        _ => "file",
    }
}

/// Special-folder kind for a lowercase directory name, if any.
fn folder_kind(name: &str) -> Option<&'static str> {
    let kind = match name {
        "src" | "source" | "sources" | "lib" | "java" | "kotlin" => "src",
        "test" | "tests" | "__tests__" | "spec" | "specs" | "testing" | "androidtest" | "uitests" => "test",
        "res" | "assets" | "resources" | "static" | "public" | "images" | "img" | "media" | "icons" | "fonts" => {
            "assets"
        }
        ".github" | ".gitlab" | ".circleci" => "github",
        "build" | "target" | "dist" | "out" | ".build" | "deriveddata" | "bin" | "obj" => "build",
        "node_modules" | "vendor" | "pods" | "packages" | ".venv" | "venv" | "third_party" | "deps" | "carthage" => {
            "deps"
        }
        "ios" | "macos" | "watchos" | "tvos" | "visionos" => "ios",
        "android" => "android",
        "gradle" | ".gradle" | "buildsrc" => "gradle",
        _ => {
            if name.ends_with(".xcassets") || name.starts_with("drawable") || name.starts_with("mipmap") {
                "assets"
            } else if name.ends_with("tests") || name.ends_with("test") {
                // Xcode targets like "AppTests" / "AppUITests".
                "test"
            } else {
                return None;
            }
        }
    };
    Some(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> &'static str {
        file_icon(Path::new(name))
    }

    fn d(name: &str, open: bool) -> &'static str {
        folder_icon(Path::new(name), open)
    }

    #[test]
    fn every_asset_is_an_svg() {
        for (path, bytes) in ASSETS {
            assert!(path.starts_with("file-icons/") && path.ends_with(".svg"));
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.starts_with("<svg") && text.contains("viewBox=\"0 0 16 16\""), "{path}");
        }
    }

    #[test]
    fn languages_by_extension() {
        assert_eq!(f("a/App.swift"), "file-icons/swift.svg");
        assert_eq!(f("Main.kt"), "file-icons/kotlin.svg");
        assert_eq!(f("Main.java"), "file-icons/java.svg");
        assert_eq!(f("main.rs"), "file-icons/rust.svg");
        assert_eq!(f("index.TSX"), "file-icons/react.svg");
        assert_eq!(f("x.d.ts"), "file-icons/typescript.svg");
        assert_eq!(f("x.mjs"), "file-icons/javascript.svg");
        assert_eq!(f("x.py"), "file-icons/python.svg");
        assert_eq!(f("x.h"), "file-icons/header.svg");
        assert_eq!(f("x.hpp"), "file-icons/cpp.svg");
        assert_eq!(f("x.scss"), "file-icons/css.svg");
        assert_eq!(f("Info.plist"), "file-icons/plist.svg");
        assert_eq!(f("logo.png"), "file-icons/image.svg");
        assert_eq!(f("unknown.zzz"), "file-icons/file.svg");
        assert_eq!(f("noext"), "file-icons/file.svg");
    }

    #[test]
    fn special_names_beat_extensions() {
        assert_eq!(f("build.gradle.kts"), "file-icons/gradle.svg");
        assert_eq!(f("settings.gradle"), "file-icons/gradle.svg");
        assert_eq!(f("package-lock.json"), "file-icons/lock.svg");
        assert_eq!(f("Cargo.lock"), "file-icons/lock.svg");
        assert_eq!(f("Package.resolved"), "file-icons/lock.svg");
        assert_eq!(f("README.md"), "file-icons/readme.svg");
        assert_eq!(f("LICENSE"), "file-icons/license.svg");
        assert_eq!(f("Dockerfile"), "file-icons/docker.svg");
        assert_eq!(f(".gitignore"), "file-icons/git.svg");
        assert_eq!(f(".env.local"), "file-icons/env.svg");
        assert_eq!(f("Podfile"), "file-icons/ruby.svg");
        assert_eq!(f("gradlew"), "file-icons/shell.svg");
    }

    #[test]
    fn folders_by_name() {
        assert_eq!(d("proj/src", false), "file-icons/folder-src.svg");
        assert_eq!(d("proj/src", true), "file-icons/folder-src-open.svg");
        assert_eq!(d("Tests", false), "file-icons/folder-test.svg");
        assert_eq!(d("DMoteUITests", true), "file-icons/folder-test-open.svg");
        assert_eq!(d("res", false), "file-icons/folder-assets.svg");
        assert_eq!(d("Assets.xcassets", false), "file-icons/folder-assets.svg");
        assert_eq!(d(".github", false), "file-icons/folder-github.svg");
        assert_eq!(d("target", false), "file-icons/folder-build.svg");
        assert_eq!(d("node_modules", false), "file-icons/folder-deps.svg");
        assert_eq!(d("ios", false), "file-icons/folder-ios.svg");
        assert_eq!(d("android", false), "file-icons/folder-android.svg");
        assert_eq!(d("gradle", true), "file-icons/folder-gradle-open.svg");
        assert_eq!(d("App.xcodeproj", true), "file-icons/xcodeproj.svg");
        assert_eq!(d("App.xcworkspace", false), "file-icons/xcworkspace.svg");
        assert_eq!(d("docs", false), "file-icons/folder.svg");
        assert_eq!(d("docs", true), "file-icons/folder-open.svg");
    }
}
