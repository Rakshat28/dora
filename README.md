# dora

**Search code the way compilers see it — not the way text editors do.**

[![crates.io](https://img.shields.io/badge/crates.io-v0.1.0-orange?style=flat-square)](https://crates.io/crates/dora)
[![license](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](./LICENSE)
[![build](https://img.shields.io/badge/build-passing-brightgreen?style=flat-square)](https://github.com/your-org/dora/actions)
[![languages](https://img.shields.io/badge/languages-7-purple?style=flat-square)](#supported-languages)
[![platform](https://img.shields.io/badge/platform-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows-lightgrey?style=flat-square)](#installation)

`dora` is a high-performance structural code search engine built on Tree-sitter. It parses source files into `Abstract Syntax` Trees and executes pattern queries against them — finding functions, types, call sites, and structural relationships that text search tools are fundamentally incapable of locating. Unlike grep, which cannot tell the difference between a function named authenticate and a comment that mentions authenticate, `dora` understands your code's grammar. Additionally, `dora` serves as a persistent "Codebase Memory" for AI coding agents. By exposing its structural index via the Model Context Protocol (`MCP`), `LLM`s can execute precise, graph-native queries directly against your codebase, retrieving exact function signatures and dependency relationships without overwhelming their context windows with raw source text.

---

## Table of Contents

- [Why Not grep?](#why-not-grep)
- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [CLI Reference](#cli-reference)
  - [search](#search-command-default)
  - [index](#index-command)
  - [lookup](#lookup-command)
  - [serve --mcp](#serve---mcp-command)
- [Query Syntax Guide](#query-syntax-guide)
  - [Basics](#basics)
  - [Captures](#captures)
  - [Predicates](#predicates)
  - [Multiple queries](#multiple-queries)
- [Usage Examples by Language](#usage-examples-by-language)
  - [Rust](#rust)
  - [Python](#python)
  - [JavaScript](#javascript)
  - [TypeScript](#typescript)
  - [Go](#go)
  - [C](#c)
  - [C++](#c-1)
  - [Auto-detection](#auto-detection)
- [The Bloom Filter Index](#the-bloom-filter-index)
- [The Persistent Structural Index](#the-persistent-structural-index)
- [Semantic Rewriting](#semantic-rewriting)
- [Interactive TUI](#interactive-tui)
- [MCP Server for AI Agents](#mcp-server-for-ai-agents)
- [Performance](#performance)
- [Architecture](#architecture)
- [Building from Source](#building-from-source)
- [Contributing](#contributing)
- [License](#license)

---

## Why Not grep?

Every text-based search tool — `grep`, `ripgrep`, `ack`, `ag` — suffers from the same fundamental blindness: they treat source code as a string. They have no concept of grammar, scope, or structure.

When you run:

```bash
rg "authenticate"
```

You get every occurrence of those 12 characters — inside function names, variable names, string literals, comments, dead code, documentation, and test fixtures alike. You get everything, and you cannot filter it without writing increasingly fragile regular expressions.

`dora` answers questions that text search cannot:

| Question | grep / ripgrep | dora |
|---|---|---|
| Find function *definitions* named `authenticate` | Returns all occurrences everywhere | Returns only `function_item` definition nodes |
| Find functions taking exactly 2 arguments | Cannot be expressed reliably | Trivial — query the parameter list child count |
| Find all `unwrap()` calls outside test modules | Cannot express scope constraints | Single query with scope predicate |
| Find structs that implement a specific trait | Multi-step, fragile, many false positives | One S-expression query |
| Rename a function at every *definition* site | Risks corrupting string literals and comments | Semantic rewriting via AST — surgical precision |
| Find all type aliases named `Result` | Returns `Result` everywhere | Returns only `type_alias_declaration` nodes |

The key insight: `dora` is to `grep` what a SQL database is to a flat text file. Both contain the same data; one understands its structure.

---

## Features

- **Structural pattern matching** via Tree-sitter S-expression queries
- **7 languages**: Rust, Python, JavaScript, TypeScript, Go, C, C++
- **Language auto-detection** per file from extension — walk mixed-language repos in one command
- **Multiple queries in one pass** — the AST is traversed exactly once regardless of how many `-q` flags you pass
- **Bloom filter pre-rejection index** — skip files that mathematically cannot contain your search term before invoking the parser
- **Persistent SQLite structural index** — extract and query all symbols (functions, structs, types, imports) across an entire codebase
- **Semantic rewriting** — surgically replace structural patterns without corrupting surrounding syntax
- **Interactive TUI** — split-pane AST visualizer with live streaming results
- **MCP server** — expose your codebase's structural graph to LLM coding agents
- **Respects `.gitignore`** — never parses `node_modules`, `target/`, or build artifacts
- **Parallel file processing** via Rayon work-stealing thread pool
- **Flat RAM profile** — memory usage is bounded by thread count, not repository size
- **Shell completions** for bash, zsh, and fish

---

## Installation

### From crates.io

```bash
cargo install dora
```

Requires Rust 1.78 or later.

### Pre-built binaries

Download from the [Releases page](https://github.com/your-org/dora/releases):

| Platform | Architecture | Binary |
|---|---|---|
| Linux | x86_64 | `dora-x86_64-unknown-linux-gnu` |
| Linux | aarch64 | `dora-aarch64-unknown-linux-gnu` |
| macOS | x86_64 | `dora-x86_64-apple-darwin` |
| macOS | Apple Silicon | `dora-aarch64-apple-darwin` |
| Windows | x86_64 | `dora-x86_64-pc-windows-msvc.exe` |

### From source

```bash
git clone https://github.com/your-org/dora
cd dora
cargo build --release
# Binary at ./target/release/dora
```

### Shell completions

```bash
# Bash
dora --generate-completions bash >> ~/.bashrc

# Zsh
dora --generate-completions zsh >> ~/.zshrc

# Fish
dora --generate-completions fish > ~/.config/fish/completions/dora.fish
```

---

## Quick Start

```bash
# Find all Rust function definitions in ./src
dora -q '(function_item name: (identifier) @fn_name)' -p ./src

# Find a specific function by name
dora -q '(function_item name: (identifier) @fn (#eq? @fn "connect"))' -p ./src

# Search Python files
dora -q '(function_definition name: (identifier) @fn)' -p . --lang python

# Auto-detect language per file — search everything at once
dora -q '(function_declaration name: (identifier) @fn_name)' -p .

# Multiple queries in a single tree-traversal pass
dora \
  -q '(function_item name: (identifier) @fn_name)' \
  -q '(struct_item name: (type_identifier) @struct_name)' \
  -p ./src --no-color

# Build a Bloom filter index for faster searches
dora index ./src

# Build both Bloom filter and persistent SQLite symbol index
dora index ./src --persist

# Look up a symbol by name in the persistent index
dora lookup --symbol parse_file --path ./src

# Launch the interactive TUI
dora -q '(function_item name: (identifier) @fn)' -p . --tui
```

**Example output:**

```
src/auth/handler.rs:42:0  [@fn_name]  "parse_token"
src/auth/handler.rs:89:0  [@fn_name]  "validate_session"
src/db/pool.rs:14:0       [@fn_name]  "connect"

Found 47 matches across 23 files in 38ms
```

---

## CLI Reference

### search command (default)

When no subcommand is given, `dora` runs a structural search.

```
dora [search] [OPTIONS] --query <S-EXPR>
```

| Flag | Type | Default | Description |
|---|---|---|---|
| `-q, --query <S-EXPR>` | String (repeatable) | required | S-expression query. Pass multiple `-q` flags for single-pass multi-query search. |
| `-p, --path <DIR>` | PathBuf | `.` | Root directory to search. Must exist and be a directory. |
| `-l, --lang <LANG>` | String | `auto` | Language: `rust`, `python`, `js`, `ts`, `go`, `c`, `cpp`, `auto`. |
| `--no-color` | bool | false | Disable ANSI color. Also respected via `NO_COLOR` env var. |
| `-Q, --quiet` | bool | false | Suppress per-match lines. Show only the summary. |
| `--stats` | bool | false | Print detailed performance diagnostics to stderr. |
| `--tui` | bool | false | Launch the interactive terminal UI. |
| `--rewrite <TEMPLATE>` | String | — | Rewrite matched captures using `@capture_name` substitution. Dry-run by default. |
| `--in-place` | bool | false | Apply rewrites to files. Requires `--rewrite`. Shows diff and prompts for confirmation. |
| `--yes` | bool | false | Skip confirmation prompt with `--in-place`. |
| `--no-update-index` | bool | false | Disable automatic incremental index updates during search. |

**Output format (stdout):**

```
src/auth/handler.rs:42:0  [@fn_name]  "parse_token"
^─────────────────────^   ^─────────^ ^────────────^
      filepath:line:col   capture     matched text
```

- Filepath: cyan
- Capture name: yellow
- Matched text: green, always in literal quotes
- Line numbers: 1-indexed
- Columns: 0-indexed byte offsets

**Summary (stderr):**

```
Found 47 matches across 23 files in 38ms
```

**Stats output (with `--stats`):**

```
--- search statistics ---
files walked:     47
files parsed:     46
files skipped:    1
matches found:    12
sieve rejected:   18
match rate:       26.09% (files with matches / files parsed)
wall time:        38ms
throughput:       1236.84 files/sec
index updated:    3 entries
```

---

### index command

Builds or updates the Bloom filter index and optionally the persistent SQLite structural index.

```
dora index <PATH> [OPTIONS]
```

| Flag | Type | Default | Description |
|---|---|---|---|
| `<PATH>` | PathBuf | required | Root directory to index. |
| `--lang <LANG>` | String | `auto` | Language filter for indexing. |
| `--persist` | bool | false | Also extract symbols and insert into the SQLite structural index. |
| `--verbose` | bool | false | Print one line per file: `indexed:`, `fresh:`, or `removed:`. |

The Bloom filter index is stored at `<PATH>/.dora-index` (bincode format).
The SQLite structural index is stored at `<PATH>/.dora-memory.db`.

Both indexes are updated incrementally — files whose mtime and size match the stored entry are skipped.

```bash
# Build Bloom filter index only
dora index ./src

# Build both indexes with verbose output
dora index ./src --persist --verbose
```

```
indexed: src/auth/handler.rs
indexed: src/db/pool.rs
fresh:   src/main.rs
removed: src/old/legacy.rs

indexed 44 files, skipped 2 fresh, removed 1 stale entries, extracted 312 symbols
index written to .dora-index
```

---

### lookup command

Queries the persistent SQLite structural index for symbols by name, prefix, kind, or language.

```
dora lookup [OPTIONS]
```

| Flag | Type | Default | Description |
|---|---|---|---|
| `--symbol <NAME>` | String | — | Exact symbol name. Mutually exclusive with `--prefix`. |
| `--prefix <PREFIX>` | String | — | Find all symbols whose name starts with PREFIX. |
| `--kind <KIND>` | String | — | Filter by kind: `function`, `struct`, `class`, `enum`, `trait`, `interface`, `type_alias`, `constant`, `variable`, `module`, `import`. |
| `--lang <LANG>` | String | — | Filter results to files of this language. |
| `-p, --path <DIR>` | PathBuf | `.` | Root directory where the index was built. |
| `--no-color` | bool | false | Disable ANSI color output. |

At least one of `--symbol` or `--prefix` is required. Both cannot be used together.

**Output format** matches structural search output exactly:

```
src/parser.rs:45:0  [@function]  "parse_file"
  signature: pub fn parse_file(path: &Path, language: &tree_sitter::Language) -> Result<(Tree, FileSource)>

Found 1 symbol in 1 file in 2ms
```

**Examples:**

```bash
# Look up an exact function name
dora lookup --symbol authenticate --path ./src

# Find all symbols starting with "handle_"
dora lookup --prefix handle_ --path ./src

# Find all structs
dora lookup --prefix "" --kind struct --path ./src

# Find Rust functions only (in a mixed-language repo)
dora lookup --prefix connect --kind function --lang rust --path .
```

---

### serve --mcp command

Starts an MCP (Model Context Protocol) server that exposes the structural index to LLM coding agents.

```
dora serve --mcp [OPTIONS]
```

| Flag | Type | Default | Description |
|---|---|---|---|
| `--mcp` | bool | required | Enable MCP server mode over JSON-RPC stdio transport. |

See [MCP Server for AI Agents](#mcp-server-for-ai-agents) for full setup instructions.

---

## Query Syntax Guide

`dora` uses Tree-sitter's S-expression pattern syntax. An S-expression is a Lisp-like notation that mirrors the shape of the syntax tree.

### Basics

**Match any node of a given type:**

```scheme
(function_item)
```

Matches every `function_item` node anywhere in the tree.

**Match a specific child:**

```scheme
(function_item name: (identifier))
```

Matches only `function_item` nodes that have a `name` field containing an `identifier` node.

**Nested patterns:**

```scheme
(impl_item
  type: (type_identifier)
  body: (declaration_list
    (function_item name: (identifier))))
```

Patterns can nest to arbitrary depth, mirroring the tree structure.

**Wildcard:**

```scheme
(function_item name: (_))
```

`(_)` matches any single node regardless of type.

### Captures

A `@capture_name` tag extracts the matched node's text and includes it in the output.

```scheme
(function_item name: (identifier) @fn_name)
```

Multiple captures per query are supported:

```scheme
(function_item
  name: (identifier) @fn_name
  parameters: (parameters) @params)
```

Each capture produces a separate result line in the output.

### Predicates

Predicates filter captures based on their text content. They appear inside the S-expression after the structural pattern.

**`#eq?` — exact equality:**

```scheme
(function_item
  name: (identifier) @fn
  (#eq? @fn "connect"))
```

**`#match?` — regular expression:**

```scheme
(function_item
  name: (identifier) @fn
  (#match? @fn "^(get|set|update)_"))
```

Matches function names starting with `get_`, `set_`, or `update_`. The regex is compiled once at query compile time and never recompiled per file.

**`#not-eq?` — negative equality:**

```scheme
(function_item
  name: (identifier) @fn
  (#not-eq? @fn "main"))
```

**`#any-of?` — match any value in a list:**

```scheme
(function_item
  name: (identifier) @fn
  (#any-of? @fn "get" "set" "delete"))
```

### Multiple queries

Pass multiple `-q` flags to run several queries in a single tree traversal. The AST is walked exactly once per file regardless of query count:

```bash
dora \
  -q '(function_item name: (identifier) @fn_name)' \
  -q '(struct_item name: (type_identifier) @struct_name)' \
  -q '(enum_item name: (type_identifier) @enum_name)' \
  -p ./src
```

Results from all queries are merged, sorted by file and position, and deduplicated.

---

## Usage Examples by Language

### Rust

```bash
# All function definitions
dora -q '(function_item name: (identifier) @fn_name)' -p ./src

# A specific function
dora -q '(function_item name: (identifier) @fn (#eq? @fn "authenticate"))' -p .

# All functions matching a naming pattern
dora -q '(function_item name: (identifier) @fn (#match? @fn "^handle_"))' -p ./src

# All struct definitions
dora -q '(struct_item name: (type_identifier) @struct_name)' -p ./src

# All enum definitions
dora -q '(enum_item name: (type_identifier) @enum_name)' -p ./src

# All trait definitions
dora -q '(trait_item name: (type_identifier) @trait_name)' -p ./src

# All impl blocks for a specific type
dora \
  -q '(impl_item type: (type_identifier) @t (#eq? @t "Config"))' \
  -p ./src

# All trait implementations (impl Trait for Type)
dora \
  -q '(impl_item trait: (type_identifier) @trait type: (type_identifier) @type)' \
  -p ./src

# All .unwrap() call sites
dora \
  -q '(call_expression function: (field_expression field: (field_identifier) @m (#eq? @m "unwrap")))' \
  -p ./src

# All use declarations (imports)
dora -q '(use_declaration) @import' -p ./src

# Functions returning a specific type
dora \
  -q '(function_item return_type: (generic_type type: (type_identifier) @t (#eq? @t "Result")) @fn)' \
  -p ./src

# All type aliases
dora -q '(type_item name: (type_identifier) @alias_name)' -p ./src

# Constants
dora -q '(const_item name: (identifier) @const_name)' -p ./src
```

### Python

```bash
# All function definitions
dora -q '(function_definition name: (identifier) @fn_name)' -p . --lang python

# Test functions only
dora \
  -q '(function_definition name: (identifier) @fn (#match? @fn "^test_"))' \
  -p . --lang python

# Class definitions
dora -q '(class_definition name: (identifier) @class_name)' -p . --lang python

# Decorated functions (e.g. @property, @staticmethod, @app.route)
dora \
  -q '(decorated_definition
        decorator: (decorator) @dec
        definition: (function_definition name: (identifier) @fn_name))' \
  -p . --lang python

# Import statements
dora -q '(import_statement) @import' -p . --lang python
dora -q '(import_from_statement) @from_import' -p . --lang python

# Class methods
dora \
  -q '(class_definition
        body: (block
          (function_definition name: (identifier) @method_name)))' \
  -p . --lang python
```

### JavaScript

```bash
# Function declarations
dora -q '(function_declaration name: (identifier) @fn_name)' -p . --lang js

# Class declarations
dora -q '(class_declaration name: (identifier) @class_name)' -p . --lang js

# Method definitions
dora -q '(method_definition name: (property_identifier) @method_name)' -p . --lang js

# Arrow functions assigned to const
dora \
  -q '(lexical_declaration
        (variable_declarator
          name: (identifier) @fn_name
          value: (arrow_function)))' \
  -p . --lang js

# Import declarations
dora -q '(import_declaration) @import' -p . --lang js

# Specific function
dora \
  -q '(function_declaration name: (identifier) @fn (#eq? @fn "authenticate"))' \
  -p . --lang js
```

### TypeScript

```bash
# Function declarations
dora -q '(function_declaration name: (identifier) @fn_name)' -p . --lang ts

# Interface declarations
dora -q '(interface_declaration name: (type_identifier) @interface_name)' -p . --lang ts

# Type aliases
dora -q '(type_alias_declaration name: (type_identifier) @type_name)' -p . --lang ts

# Class declarations
dora -q '(class_declaration name: (identifier) @class_name)' -p . --lang ts

# Enum declarations
dora -q '(enum_declaration name: (identifier) @enum_name)' -p . --lang ts

# Import declarations
dora -q '(import_declaration) @import' -p . --lang ts

# Generic functions
dora \
  -q '(function_declaration
        name: (identifier) @fn_name
        type_parameters: (type_parameters))' \
  -p . --lang ts

# TSX component definitions (functions returning JSX)
dora -q '(function_declaration name: (identifier) @component)' -p . --lang ts
```

### Go

```bash
# Function declarations (not methods)
dora -q '(function_declaration name: (identifier) @fn_name)' -p . --lang go

# Method declarations (with receiver)
dora -q '(method_declaration name: (field_identifier) @method_name)' -p . --lang go

# Struct type declarations
dora \
  -q '(type_declaration (type_spec name: (type_identifier) @type_name))' \
  -p . --lang go

# Interface type declarations
dora \
  -q '(type_declaration
        (type_spec
          name: (type_identifier) @interface_name
          type: (interface_type)))' \
  -p . --lang go

# Import declarations
dora -q '(import_declaration) @import' -p . --lang go

# Functions with a specific receiver type
dora \
  -q '(method_declaration
        receiver: (parameter_list
          (parameter_declaration type: (type_identifier) @recv (#eq? @recv "Config")))
        name: (field_identifier) @method_name)' \
  -p . --lang go
```

### C

```bash
# Function definitions
dora \
  -q '(function_definition
        declarator: (function_declarator
          declarator: (identifier) @fn_name))' \
  -p . --lang c

# Typedef names
dora -q '(type_definition declarator: (type_identifier) @type_name)' -p . --lang c

# Struct declarations
dora -q '(struct_specifier name: (type_identifier) @struct_name)' -p . --lang c

# A specific function
dora \
  -q '(function_definition
        declarator: (function_declarator
          declarator: (identifier) @fn (#eq? @fn "main")))' \
  -p . --lang c

# Include directives
dora -q '(preproc_include) @include' -p . --lang c

# Macro definitions
dora -q '(preproc_def name: (identifier) @macro_name)' -p . --lang c
```

### C++

```bash
# Function definitions (free functions)
dora \
  -q '(function_definition
        declarator: (function_declarator
          declarator: (identifier) @fn_name))' \
  -p . --lang cpp

# Class declarations
dora -q '(class_specifier name: (type_identifier) @class_name)' -p . --lang cpp

# Struct declarations
dora -q '(struct_specifier name: (type_identifier) @struct_name)' -p . --lang cpp

# Template declarations
dora -q '(template_declaration) @template' -p . --lang cpp

# Namespace declarations
dora -q '(namespace_definition name: (namespace_identifier) @ns_name)' -p . --lang cpp

# Constructor definitions
dora \
  -q '(function_definition
        declarator: (function_declarator
          declarator: (qualified_identifier) @ctor_name))' \
  -p . --lang cpp
```

### Auto-detection

When `--lang auto` is used (the default), `dora` detects the grammar per file from its extension and walks all supported extensions simultaneously. The query is compiled against every grammar at startup — grammars for which the query fails to compile are silently skipped.

```bash
# Search all source files — auto-detects language per file
# function_declaration compiles for JS, TS, Go, C, C++ — not Rust or Python
dora -q '(function_declaration name: (identifier) @fn_name)' -p .

# function_item only exists in Rust — auto mode searches only .rs files
dora -q '(function_item name: (identifier) @fn_name)' -p .

# identifier exists in every grammar — auto mode searches everything
dora -q '(identifier) @id' -p . --quiet
```

**Extension mapping:**

| `--lang` flag | Extensions |
|---|---|
| `rust` | `.rs` |
| `python` | `.py`, `.pyi` |
| `js` | `.js`, `.mjs`, `.cjs` |
| `ts` | `.ts`, `.tsx`, `.mts`, `.cts` |
| `go` | `.go` |
| `c` | `.c`, `.h` |
| `cpp` | `.cpp`, `.cc`, `.hpp`, `.hxx`, `.cxx`, `.h` |

> **Note on `.h` files:** In auto mode, `.h` files are parsed with the C grammar. To parse them with the C++ grammar, use `--lang cpp` explicitly.

---

## The Bloom Filter Index

The Bloom filter index is a pre-parse rejection sieve. Files that *mathematically cannot* contain your search term are skipped entirely before tree-sitter is ever invoked.

### How it works

1. **Index phase**: Each file's source bytes are broken into all consecutive 3-byte windows (trigrams). `"hello"` → `[hel, ell, llo]`. Unique trigrams are inserted into a per-file Bloom filter — a 4096-bit (512-byte) bit array using two FNV-1a hash functions.

2. **Query phase**: String literals in predicates (`#eq? @fn "authenticate"`) are decomposed into trigrams at query compile time.

3. **Rejection phase**: Before invoking tree-sitter, the file's Bloom filter is checked. If any required trigram is absent, the file is skipped in under 0.003ms. **Zero false negatives are guaranteed** — a file containing the search term will always pass the filter.

### Building the index

```bash
# Build the Bloom filter index
dora index ./src

# Rebuild verbosely
dora index ./src --verbose
```

### How the search uses it

The search pipeline automatically loads and uses the index when it exists:

```bash
# First search (no index): parses all 47 files
dora -q '(function_item name: (identifier) @fn (#eq? @fn "connect"))' -p ./src
# Found 1 match across 47 files in 156ms

# After building the index: most files skipped
dora index ./src
dora -q '(function_item name: (identifier) @fn (#eq? @fn "connect"))' -p ./src --stats
# sieve rejected: 41  ← 41 files skipped before parsing
# Found 1 match across 6 files in 23ms
```

The index updates incrementally during search — stale entries for modified files are refreshed automatically. Use `--no-update-index` to disable this.

---

## The Persistent Structural Index

Beyond the Bloom filter, `dora` can build a full SQLite database of all symbols in your codebase — function definitions, struct definitions, type aliases, import statements, trait implementations, and more. This is the **persistent structural index**.

### Building it

```bash
dora index ./src --persist
```

This extracts symbols from every file and inserts them into `.dora-memory.db`. The schema:

```sql
-- One row per indexed file
files(id, path, mtime, language, indexed_at)

-- One row per extracted symbol
symbols(id, file_id, kind, name, start_line, start_col, end_line, end_col, signature)
```

Supported symbol kinds: `function`, `method`, `struct`, `enum`, `trait`, `interface`, `type_alias`, `constant`, `variable`, `class`, `module`, `import`.

### Querying it

```bash
# Exact name lookup
dora lookup --symbol authenticate --path ./src

# Prefix search
dora lookup --prefix handle_ --path ./src

# Filter by kind
dora lookup --prefix "" --kind struct --path ./src

# Filter by language in a mixed-language repo
dora lookup --prefix connect --lang rust --path .

# Find all functions whose name matches a pattern (prefix)
dora lookup --prefix parse_ --kind function --path ./src
```

**Output:**

```
src/auth/handler.rs:42:0  [@function]  "authenticate"
  signature: pub fn authenticate(user: &str, password: &str) -> bool

src/auth/token.rs:18:0  [@function]  "authenticate_token"
  signature: pub fn authenticate_token(token: &str) -> Result<Claims>

Found 2 symbols in 2 files in 2ms
```

The lookup command is significantly faster than structural search for name-based queries because it queries an indexed SQL table rather than walking and parsing the filesystem.

---

## Semantic Rewriting

`dora` can surgically rewrite code by replacing structural patterns without touching surrounding syntax. This is fundamentally safer than `sed` — it targets only AST nodes matching the query, never string literals or comments that happen to contain the same text.

### Dry run (default)

```bash
dora \
  -q '(function_item name: (identifier) @fn_name (#eq? @fn_name "old_name"))' \
  --rewrite 'new_name' \
  -p ./src
```

Prints a colored unified diff without modifying any files.

### Apply in place

```bash
# Shows diff, prompts for confirmation
dora \
  -q '(function_item name: (identifier) @fn_name (#eq? @fn_name "old_name"))' \
  --rewrite 'new_name' \
  --in-place \
  -p ./src

# Skip confirmation prompt
dora \
  -q '(function_item name: (identifier) @fn_name (#eq? @fn_name "old_name"))' \
  --rewrite 'new_name' \
  --in-place --yes \
  -p ./src
```

### Template syntax

Use `@capture_name` in the template to substitute captured text:

```bash
# Rename a function: @fn_name is replaced by the captured function name
--rewrite 'renamed_@fn_name'

# Prefix all test functions
dora \
  -q '(function_item name: (identifier) @fn (#match? @fn "^test_"))' \
  --rewrite 'legacy_@fn' \
  -p ./src
```

### How it works

Rewrites are applied in **reverse byte order** — edits at the end of a file are applied first so that earlier byte offsets remain valid throughout the process. Each rewrite is atomic (temp file + rename) to prevent partial writes from corrupting files.

---

## Interactive TUI

Launch the interactive terminal UI with `--tui` for a split-pane explorer with live streaming results:

```bash
dora -q '(function_item)' -p . --tui
```

```
┌─ Files ──────────────────┐┌─ Code ─────────────────────────────────────────┐
│ src/auth/handler.rs  3   ││ 41 │                                            │
│ src/auth/token.rs    1   ││ 42 │ ▶ pub fn parse_token(input: &str) -> Token │
│ src/db/pool.rs       1   ││ 43 │     let raw = input.trim();                │
│ src/main.rs          5   ││ 44 │     Token::from_str(raw)                   │
└──────────────────────────┘└────────────────────────────────────────────────┘
┌─ AST ───────────────────────────────────────────────────────────────────────┐
│ ▼ function_item [42:0 → 58:1]                                               │
│     name: identifier "parse_token"  ●                                       │
│   ▼ parameters: parameters                                                  │
│       parameter: identifier "input"                                         │
└─────────────────────────────────────────────────────────────────────────────┘
  [↑↓/jk] navigate   [Enter] expand/collapse   [Tab] focus pane   [q] quit
```

**Key bindings:**

| Key | Action |
|---|---|
| `/` or typing | Update the live query |
| `j` / `k` or arrows | Navigate file list (File Tree pane) or scroll (Code/AST pane) |
| `Tab` | Cycle focus: File Tree → Code View → AST View |
| `Enter` | Submit query immediately (bypasses debounce) / expand-collapse AST node |
| `g` / `G` | Jump to top / bottom of AST pane |
| `<` / `>` | Shrink / grow the active pane |
| `q` or `Esc` | Quit and restore terminal |

**Features:**
- Results stream in live as background threads process files
- 300ms debounce on keystrokes — search starts automatically when you pause typing
- Searches can be cancelled by typing a new query
- Code view auto-scrolls to the first match in the selected file
- Matched nodes marked with `●` in the AST pane
- Active pane has a bold border
- Terminal resize reflows the layout immediately with no artifacts

---

## MCP Server for AI Agents

`dora` exposes a [Model Context Protocol](https://modelcontextprotocol.io) server so LLM coding agents (Claude Code, Continue, Cursor, etc.) can query your codebase structurally.

### Why LLMs need structural search

LLM coding agents have limited context windows. When an agent tries to understand a large codebase by reading files via `grep` or raw file reads, it burns context tokens on irrelevant content and still has no structural understanding of the architecture.

With the `dora` MCP server, an agent can ask:

> *"What is the exact type signature of the function handling user authentication?"*

and receive a precise, structured answer in milliseconds — using a fraction of the context tokens that reading source files would require. This reduces hallucinations (the agent sees real signatures, not guesses) and dramatically improves the accuracy of refactoring, debugging, and code-generation tasks.

### Setup

**Step 1: Build the persistent index**

```bash
dora index --persist /path/to/your/repo
```

**Step 2: Start the MCP server**

```bash
dora serve --mcp
```

**Step 3: Configure your MCP client**

Add to your `.mcp.json` (or equivalent client configuration):

```json
{
  "mcpServers": {
    "dora": {
      "command": "dora",
      "args": ["serve", "--mcp"],
      "cwd": "/path/to/your/repo"
    }
  }
}
```

### Available MCP tools

The server exposes two tools via JSON-RPC over stdio:

**`search_ast`** — Run a live S-expression structural search:

```json
{
  "tool": "search_ast",
  "arguments": {
    "query": "(function_item name: (identifier) @fn (#eq? @fn \"authenticate\"))"
  }
}
```

Returns matching `MatchResult` objects with file paths, line/column positions, and captured text. This runs the full tree-sitter pipeline in real time.

**`lookup_symbol`** — Query the persistent SQLite index for a symbol by name:

```json
{
  "tool": "lookup_symbol",
  "arguments": {
    "name": "authenticate"
  }
}
```

Returns `SymbolRow` objects including the symbol kind, file path, position, and full signature text. This is O(log n) against the indexed database — instantaneous even for million-line codebases.

### Example agent interaction

Instead of:
> Agent reads 15 files to find where authentication is handled, burning 12,000 tokens

With dora MCP:
> Agent calls `lookup_symbol("authenticate")` → receives 2 results with signatures in <5ms, uses 150 tokens

---

## Performance

`dora` is engineered for sub-second query latency on repositories with millions of lines of code.

### Benchmark results

| Benchmark | Result | Hardware |
|---|---|---|
| Single file parse + query (100 functions) | ~180µs | Apple M2, release |
| 10,000-file Rust repository | **<1,000ms** | 8-core modern laptop |
| Parallel search, 100 files, 20 fn each | ~45ms | 8-core, Rayon |
| Query compilation (Rust grammar, 1 query) | ~12µs | — |
| Query compilation (all 7 languages, auto mode) | ~85µs | — |
| Bloom filter rejection check per file | <0.003ms | — |
| Symbol lookup (SQLite, indexed) | <2ms | 100k+ symbol corpus |

### Comparison with text search

| Tool | Type | 10k-file Rust repo | Accuracy |
|---|---|---|---|
| `ripgrep` | Text (regex) | ~9ms | Many false positives |
| `dora` (no index) | Structural | ~380ms | Zero false positives |
| `dora` (with index) | Structural + sieve | ~85ms | Zero false positives |
| `dora lookup` | SQLite index | <2ms | Exact symbol match |

`dora` without an index is ~40× slower than `ripgrep` because it does fundamentally more work — it parses every file into a full syntax tree. With the Bloom filter index, that gap narrows to ~9× for queries with string literal predicates. For symbol lookups via the SQLite index, it is faster than ripgrep.

### Why it's fast

Every layer of the pipeline has a targeted optimization:

| Layer | Optimization | Effect |
|---|---|---|
| Pre-parse | Bloom filter trigram sieve | Skips files that cannot match before tree-sitter is invoked |
| Query compilation | BitSet `potential_kinds` filtering | Skips `match_node` evaluation for the vast majority of tree nodes |
| Regex predicates | Pre-compiled `Arc<Regex>` at startup | Zero per-file regex compilation |
| Multi-query | Single-pass automaton | One tree traversal regardless of `-q` flag count |
| File I/O | `memmap2` for files ≥ 1MB | Avoids heap allocation of large source strings |
| Parallelism | Rayon work-stealing thread pool | Saturates all CPU cores with zero lock contention during parse |
| Parser lifecycle | Thread-local parser pool | One `Parser` per thread, never reallocated per file |
| Memory | Ephemeral tree lifecycle | RAM bounded by thread count, not repository size |

---

## Architecture

```
CLI args
   │
   ▼
SearchConfig ────────────────────────────────────────────────┐
   │                                                          │
   ▼                                                          │
Arc<MultiCompiledQuery> (compiled once, shared across threads)│
   │                                                          │
   ▼                                                          │
WalkBuilder → file paths → par_bridge() (Rayon)               │
                               │                              │
                               ▼                              │
                    ┌──────────────────────┐                  │
                    │   Bloom filter sieve  │                  │
                    │   (skip if rejected)  │                  │
                    └──────────────────────┘                  │
                               │                              │
                               ▼                              │
                    thread_local! Parser pool                  │
                    (set_language per file)                    │
                               │                              │
                               ▼                              │
                    tree-sitter CST (FileSource)               │
                               │                              │
                               ▼                              │
                    QueryCursor DFS traversal                  │
                    BitSet kind pre-filter                     │
                    Arc<Regex> predicate evaluation            │
                               │                              │
                               ▼                              │
                    Vec<MatchResult> (extracted)               │
                               │                              │
                    [Tree + source dropped] ◄──────────────────┘
                               │
                               ▼
                    Arc<Mutex<Vec<MatchResult>>>
                               │
                    sort → dedup → print
```

**Key design decisions:**

- **Query compiled before walk**: `Arc<MultiCompiledQuery>` is built once and shared with zero per-thread recompilation
- **Ephemeral tree lifecycle**: `Tree` and source bytes are dropped immediately after `extract_matches` returns — RAM stays flat
- **Single-pass multi-query**: multiple `-q` flags are merged into one DFS traversal per file
- **BitSet pre-filtering**: each node's numeric kind ID is checked against a `HashSet<u16>` before pattern evaluation
- **Cooperative cancellation**: the TUI search worker checks a `CancellationToken` before each file, enabling instant response to new queries

---

## Building from Source

**Prerequisites:** Rust 1.78+, a C compiler (for Tree-sitter grammar compilation)

```bash
git clone https://github.com/your-org/dora
cd dora

# Debug build
cargo build

# Release build (significantly faster binary)
cargo build --release

# Run tests
cargo test --all-features

# Lint
cargo clippy --all-features -- -D warnings

# Format
cargo fmt
```

**Key dependencies:**

| Crate | Version | Purpose |
|---|---|---|
| `tree-sitter` | 0.22 | Incremental parsing core |
| `tree-sitter-{rust,python,...}` | 0.21 | Language grammars (compiled from C) |
| `rayon` | 1.10 | Work-stealing parallel iterator |
| `ignore` | 0.4 | Gitignore-aware directory walker |
| `clap` | 4 | CLI argument parsing |
| `regex` | 1 | Pre-compiled predicate evaluation |
| `bincode` | 2 | Bloom filter index serialization |
| `rusqlite` | 0.31 | Persistent structural index (bundled SQLite) |
| `memmap2` | 0.9 | Memory-mapped file reading |
| `ratatui` | 0.27 | Terminal UI rendering |
| `tokio` | 1 | Async runtime for TUI event loop |
| `similar` | 2 | Unified diff generation for `--rewrite` |

---

## Contributing

Contributions are welcome. The project is tracked issue-by-issue across 15 milestones. See the [open issues](https://github.com/your-org/dora/issues) for the full roadmap.

**Before opening a PR:**

```bash
cargo fmt
cargo clippy --all-features -- -D warnings
cargo test --all-features
```

All three must pass cleanly. The CI pipeline enforces `#![deny(warnings)]` and `#![warn(clippy::pedantic)]` across the entire codebase.

**Adding a new language:**

1. Add `tree-sitter-<lang>` to `Cargo.toml`
2. Add a variant to the `Language` enum in `src/types.rs`
3. Add extension mapping in `src/walker.rs::extensions_for_language`
4. Add grammar arm in `src/parser.rs::get_language`
5. Add detection arm in `src/parser.rs::detect_language`
6. Add the language to `src/parser.rs::get_all_languages`
7. Add `"<lang>"` to `resolve_lang` and `validate` in `src/main.rs`
8. Add a fixture file in `tests/fixtures/`
9. Add integration tests following the pattern of existing language tests

---

## License

MIT — see [LICENSE](./LICENSE).