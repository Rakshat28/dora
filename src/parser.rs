use std::{cell::RefCell, fs, path::Path};

use tree_sitter::{Parser, Tree};

use crate::types::{AppError, Result};

thread_local! {
    // Each Rayon worker reuses a single Parser instance so we avoid repeated
    // allocation and language setup on every file.
    static RUST_PARSER: RefCell<Parser> = RefCell::new({
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::language())
            .expect("failed to configure Rust parser");
        parser
    });
}

pub fn parse_file(path: &Path) -> Result<(Tree, String)> {
    let source = fs::read_to_string(path)?;

    let tree = RUST_PARSER.with(|parser_cell| {
        let mut parser = parser_cell.borrow_mut();
        parser.parse(&source, None)
    });

    match tree {
        Some(tree) => Ok((tree, source)),
        None => Err(AppError::ParseError(format!("failed to parse {}", path.display()))),
    }
}
