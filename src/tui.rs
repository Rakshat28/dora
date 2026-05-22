#[allow(clippy::missing_errors_doc, clippy::implicit_hasher)]
pub fn run_tui(
    _config: &crate::types::SearchConfig,
    _compiled_queries: &std::sync::Arc<
        std::collections::HashMap<
            crate::types::Language,
            std::sync::Arc<crate::query::MultiCompiledQuery>,
        >,
    >,
) -> crate::types::Result<()> {
    eprintln!("tui mode: not yet implemented");
    Ok(())
}
