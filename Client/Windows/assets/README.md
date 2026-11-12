# Chat syntax data

`syntaxes.packdump` is generated offline with syntect 5.3.0, pinned in the
standalone tool's Cargo.lock. It is embedded in the client; no runtime grammar
or theme downloads occur. Regenerate from the repository root:

```
cargo run --locked --manifest-path Client/Windows/tools/syntax-dump/Cargo.toml
```

The tool keeps selected bundled grammars and their context dependencies.
Because syntect does not expose linked context indices publicly, the tool reads
those indices from Debug output, preserving original indices with empty stubs.
This is tied to the pinned syntect version. Before writing, it checks unresolved
references and compares scope operations with the complete bundled set on a
mixed-language sample. Review this machinery when upgrading syntect.

Sources:

- [syntect v5.3.0](https://github.com/trishume/syntect/tree/v5.3.0), MIT;
  notice retained in `SYNTECT-LICENSE.txt`.
- Its bundled Sublime grammars originate in
  [sublimehq/Packages fa6b8629c95041bf262d4c1dab95c456a0530122](https://github.com/sublimehq/Packages/tree/fa6b8629c95041bf262d4c1dab95c456a0530122).
  Notices retained in `SUBLIME-PACKAGES-LICENSE.txt` and
  `RUST-GRAMMAR-LICENSE.txt`.
- `syntaxes/TOML.sublime-syntax` and `syntaxes/TypeScript.sublime-syntax` are
  small grammars authored for RustTalk. TypeScript extends bundled JavaScript;
  it is intentionally basic, not a full TypeScript/TSX parser.

The generated dump is 126,325 bytes (including TOML and TypeScript). This is
asset size, **not** a measured executable-size increase.

## Dependency rationale

The only new direct dependency is syntect 5.3 (locked to 5.3.0), an established
MIT-licensed syntax highlighter. Reimplementing 16 language grammars would be
larger and less reliable. The client enables parsing, dump loading and the
pure-Rust fancy-regex backend; no Oniguruma/C dependency, default grammar set,
YAML loader or default theme set is linked into the client. New lockfile
packages are syntect, fancy-regex and fnv; other transitive dependencies were
already present. No application unsafe code is added. Syntect exposes an unsafe
bitflags constructor; this implementation does not call it. This is not an
independent audit of all transitive code.

## Parser/security review notes (#23)

A peer can send pathological delimiters, deeply nested markup, long code lines,
or regex worst cases. The renderer handles at most 16 KiB (UTF-8 boundary safe),
256 blocks, eight inline levels, a 128 KiB delimiter-scan budget, and 1,024 final
format sections; excess formatting falls back to literal text. This is a small
chat dialect, not CommonMark: flat lists/quotes, three-backtick fences at column
zero, simple paired inline delimiters. Unclosed fences consume the remainder as
code. Plain preview text uses the same parser and leaves code content intact.

The per-client LRU holds 64 entries keyed by message ID **and content**; edits
cannot reuse stale highlighting. Each entry is bounded by the limits above.
Highlighting uses one worker and capacity-one channels, with only one job in
flight. The UI never waits for regex work. Until a result arrives, code is plain.
Each code block is limited to 4 KiB, 128 lines and 256 bytes per line. A 30 ms
cooperative budget is checked between lines for the whole message; it cannot
interrupt a single regex call. A slow call can delay highlighting, but not UI
rendering or create an unbounded job/thread queue. Fancy-regex also has its own
backtracking limit. No hard wall-clock CPU deadline is claimed.

HTML, images and links are rendered only through labels, without URL handlers,
HTML interpretation, file reads or network requests. Copy buttons copy code
only. Compact and cozy modes use the same renderer. No wire-format changes.

Validation: client parser/highlighter/cache/composer tests and existing client
tests, plus generator scope comparison. Native visual QA and Windows execution
were not performed. Release executable-size delta was not measured; no baseline
checkout was created. An independent parser/security review remains required.

`make audit` on 2026-11-12 reported four vulnerabilities already in the baseline:
RUSTSEC-2026-0459 (libsqlite3-sys), RUSTSEC-2026-0194 and RUSTSEC-2026-0195
(quick-xml), RUSTSEC-2026-0293 (ringbuf), plus 12 warnings. No advisory was
reported against the three new packages. Existing warnings include unmaintained
bincode, also used by syntect's embedded trusted dump loader.
