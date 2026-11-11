//! Finds the languages: every file in `locales/`, and the privacy notices in
//! `templates/privacy/`, compiled into the binary by code. A language is added by adding
//! its file, with no list in the code to change; `src/i18n` reads them.

use std::{env, fmt::Write, fs, path::Path};

fn main() {
    let root = env::var("CARGO_MANIFEST_DIR").expect("cargo sets it");
    let root = Path::new(&root);
    let locales = files(&root.join("locales"), "toml");
    let notices = files(&root.join("templates/privacy"), "html");

    let mut out = String::from("// Written by build.rs from locales/ and templates/privacy/.\n\n");
    out.push_str("/// Every language's file, by code, in the order of the codes.\n");
    out.push_str("pub(super) const FILES: &[(&str, &str)] = &[\n");
    for (code, path) in &locales {
        writeln!(out, "    ({code:?}, include_str!({path:?})),").unwrap();
    }
    out.push_str("];\n\n/// The privacy notices that languages have, by code.\n");
    out.push_str("pub(super) const NOTICES: &[(&str, &str)] = &[\n");
    for (code, path) in &notices {
        writeln!(out, "    ({code:?}, include_str!({path:?})),").unwrap();
    }
    // Tests name languages, as `Lang::NB`; the site never does.
    out.push_str("];\n\n#[cfg(test)]\n#[allow(dead_code)]\nimpl Lang {\n");
    for (index, (code, _)) in locales.iter().enumerate() {
        let name = code.to_ascii_uppercase().replace('-', "_");
        writeln!(out, "    pub const {name}: Lang = Lang({index});").unwrap();
    }
    out.push_str("}\n");

    let dest = Path::new(&env::var("OUT_DIR").expect("cargo sets it")).join("languages.rs");
    fs::write(dest, out).expect("writing languages.rs");
    for dir in ["locales", "templates/privacy"] {
        println!("cargo::rerun-if-changed={dir}");
    }
}

/// The files in `dir` ending in `.{extension}`, by the name before it, sorted.
fn files(dir: &Path, extension: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .map(|path| {
            let code = path.file_stem().unwrap().to_string_lossy().into_owned();
            let ok = !code.is_empty()
                && code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            assert!(
                ok,
                "{}: a language's code is lower case, as en-us",
                path.display()
            );
            (code, path.display().to_string())
        })
        .collect();
    found.sort();
    // `LangSet` keeps a set of them in the bits of a u64.
    assert!(
        found.len() <= 64,
        "{}: more languages than fit",
        dir.display()
    );
    found
}
