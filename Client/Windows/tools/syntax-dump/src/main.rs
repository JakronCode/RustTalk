//! Builds the curated syntax set for chat code blocks from syntect's bundled
//! grammars plus `assets/syntaxes/*.sublime-syntax`, and writes it to
//! `assets/syntaxes.packdump`. Before writing, compare token scopes with the
//! original bundled set to detect broken context references.

use std::collections::BTreeSet;
use std::path::Path;

use syntect::parsing::syntax_definition::SyntaxDefinition;
use syntect::parsing::{SyntaxSet, SyntaxSetBuilder};

/// Grammars the client offers (dependencies such as JavaDoc are added automatically).
const WANTED: &[&str] = &[
    "Plain Text",
    "Rust",
    "C",
    "C++",
    "C#",
    "Java",
    "JavaScript",
    "Python",
    "Go",
    "JSON",
    "YAML",
    "Bourne Again Shell (bash)",
    "SQL",
    "HTML",
    "CSS",
];

/// Syntaxes a definition links to. The bundled set is already linked, so its
/// references are context ids whose syntax index only shows in the Debug output.
fn references(def: &SyntaxDefinition) -> BTreeSet<usize> {
    let dbg = format!("{:?}", def.contexts);
    dbg.split("syntax_index: ")
        .skip(1)
        .map(|s| {
            s.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("syntax index")
        })
        .collect()
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundled = SyntaxSet::load_defaults_newlines().into_builder();
    let defs = bundled.syntaxes();

    let mut keep: BTreeSet<usize> = WANTED
        .iter()
        .map(|name| {
            defs.iter()
                .position(|d| d.name == *name)
                .unwrap_or_else(|| panic!("{name} not bundled"))
        })
        .collect();
    loop {
        let more: BTreeSet<usize> = keep
            .iter()
            .flat_map(|&i| references(&defs[i]))
            .filter(|i| !keep.contains(i))
            .collect();
        if more.is_empty() {
            break;
        }
        keep.extend(more);
    }

    // Unused grammars become empty stubs so the context ids of the kept ones stay valid.
    let mut builder = SyntaxSetBuilder::new();
    for (i, def) in defs.iter().enumerate() {
        if keep.contains(&i) {
            builder.add(def.clone());
        } else {
            let stub = format!("%YAML 1.2\n---\nname: unused-{i}\nhidden: true\nscope: text.unused.n{i}\ncontexts:\n  main: []\n");
            builder.add(SyntaxDefinition::load_from_str(&stub, true, None).expect("stub"));
        }
    }
    let own = root.join("assets/syntaxes");
    let mut files: Vec<_> = std::fs::read_dir(&own)
        .expect("assets/syntaxes")
        .map(|e| e.expect("dir entry").path())
        .collect();
    files.sort();
    for path in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "sublime-syntax"))
    {
        let src = std::fs::read_to_string(path).expect("read grammar");
        builder.add(
            SyntaxDefinition::load_from_str(&src, true, None)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        );
    }
    let set = builder.build();
    let unlinked = set.find_unlinked_contexts();
    assert!(unlinked.is_empty(), "unresolved references: {unlinked:?}");

    let original = SyntaxSet::load_defaults_newlines();
    let sample = "// comment\n/* comment */\n/** doc */\nclass Example { public static void main() { return 42; } }\nfn main() { let s = \"hello\"; }\n# comment\n[table]\nkey = true\n<script>const x = 1;</script>\n<style>body { color: red; }</style>\nSELECT * FROM users;\n";
    for name in WANTED {
        let mut before = syntect::parsing::ParseState::new(
            original.find_syntax_by_name(name).expect("bundled syntax"),
        );
        let mut after = syntect::parsing::ParseState::new(
            set.find_syntax_by_name(name).expect("curated syntax"),
        );
        for line in sample.split_inclusive('\n') {
            assert_eq!(
                before.parse_line(line, &original).expect("original parse"),
                after.parse_line(line, &set).expect("curated parse"),
                "{name}: {line}"
            );
        }
    }
    let out = root.join("assets/syntaxes.packdump");
    syntect::dumps::dump_to_uncompressed_file(&set, &out).expect("write dump");
    let kept: Vec<&str> = set
        .syntaxes()
        .iter()
        .filter(|s| !s.hidden)
        .map(|s| s.name.as_str())
        .collect();
    println!(
        "{} ({} bytes): {}",
        out.display(),
        std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
        kept.join(", ")
    );
}
