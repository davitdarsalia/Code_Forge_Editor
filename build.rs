const GRAMMARS: &[&str] = &["openstep_plist", "xcconfig", "proguard", "gitignore"];

fn main() {
    let mut build = cc::Build::new();
    build.std("c11").warnings(false);
    for name in GRAMMARS {
        let src = std::path::Path::new("grammars").join(name).join("src");
        build.include(&src).file(src.join("parser.c"));
        println!("cargo:rerun-if-changed={}", src.join("parser.c").display());
    }
    build.compile("forge_grammars");
}
