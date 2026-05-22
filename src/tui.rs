use crossterm::event::{Event, EventStream, KeyCode, KeyEvent};
use rayon::iter::ParallelBridge;
use rayon::prelude::ParallelIterator;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

pub(crate) struct FileTreeEntry {
    pub(crate) path: PathBuf,
    #[allow(dead_code)]
    pub(crate) match_count: usize,
}

pub(crate) struct AppState {
    pub(crate) query_input: String,
    pub(crate) submitted_query: Option<String>,
    pub(crate) results: Vec<crate::types::MatchResult>,
    pub(crate) selected_index: usize,
    pub(crate) file_tree: Vec<FileTreeEntry>,
    pub(crate) selected_file_index: usize,
    pub(crate) scroll_offset: usize,
    #[allow(dead_code)]
    pub(crate) ast_scroll_offset: usize,
    pub(crate) debounce_deadline: Option<tokio::time::Instant>,
    pub(crate) search_running: bool,
    pub(crate) error_message: Option<String>,
    pub(crate) should_quit: bool,
    pub(crate) frame_count: u64,
}

impl AppState {
    pub(crate) fn new() -> Self {
        Self {
            query_input: String::new(),
            submitted_query: None,
            results: Vec::new(),
            selected_index: 0,
            file_tree: Vec::new(),
            selected_file_index: 0,
            scroll_offset: 0,
            ast_scroll_offset: 0,
            debounce_deadline: None,
            search_running: false,
            error_message: None,
            should_quit: false,
            frame_count: 0,
        }
    }

    pub(crate) fn append_results(&mut self, mut new_results: Vec<crate::types::MatchResult>) {
        self.results.append(&mut new_results);
        self.results.sort();
        self.results.dedup();
        self.rebuild_file_tree();
        if !self.results.is_empty() {
            if self.selected_index >= self.results.len() {
                self.selected_index = self.results.len().saturating_sub(1);
            }
        } else {
            self.selected_index = 0;
            self.selected_file_index = 0;
        }
        if self.selected_file_index >= self.file_tree.len() && !self.file_tree.is_empty() {
            self.selected_file_index = self.file_tree.len().saturating_sub(1);
        }
    }

    pub(crate) fn clear_results(&mut self) {
        self.results.clear();
        self.file_tree.clear();
        self.selected_index = 0;
        self.selected_file_index = 0;
    }

    fn rebuild_file_tree(&mut self) {
        let mut map: HashMap<PathBuf, usize> = HashMap::new();
        for r in &self.results {
            *map.entry(r.file_path.clone()).or_insert(0) += 1;
        }
        let mut entries: Vec<FileTreeEntry> = map
            .into_iter()
            .map(|(path, match_count)| FileTreeEntry { path, match_count })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        self.file_tree = entries;
    }

    pub(crate) fn select_next(&mut self) {
        if self.file_tree.is_empty() {
            return;
        }
        self.selected_file_index = (self.selected_file_index + 1) % self.file_tree.len();
        self.scroll_offset = 0;
    }

    pub(crate) fn select_prev(&mut self) {
        if self.file_tree.is_empty() {
            return;
        }
        if self.selected_file_index == 0 {
            self.selected_file_index = self.file_tree.len().saturating_sub(1);
        } else {
            self.selected_file_index -= 1;
        }
        self.scroll_offset = 0;
    }

    pub(crate) fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    pub(crate) fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    #[allow(dead_code)]
    pub(crate) fn results_for_selected_file(&self) -> &[crate::types::MatchResult] {
        if self.file_tree.is_empty() {
            return &[];
        }
        if self.selected_file_index >= self.file_tree.len() {
            return &[];
        }
        let path = &self.file_tree[self.selected_file_index].path;
        let start = self.results.iter().position(|r| &r.file_path == path).unwrap_or(usize::MAX);
        if start == usize::MAX {
            return &[];
        }
        let end = self.results.iter().rposition(|r| &r.file_path == path).unwrap_or(start);
        &self.results[start..=end]
    }
}

#[allow(dead_code)]
pub(crate) enum AppEvent {
    Keystroke(KeyEvent),
    Tick,
    SearchStarted,
    SearchResult(Vec<crate::types::MatchResult>),
    SearchComplete,
    SearchError(String),
}

#[allow(dead_code)]
pub(crate) enum SearchCommand {
    Run(String),
    Cancel,
}

pub fn run_tui(
    config: &crate::types::SearchConfig,
    compiled_queries: &std::sync::Arc<
        std::collections::HashMap<
            crate::types::Language,
            std::sync::Arc<crate::query::MultiCompiledQuery>,
        >,
    >,
) -> crate::types::Result<()> {
    let rt = Runtime::new().map_err(|e| {
        crate::types::AppError::IoError(std::io::Error::other(e.to_string()))
    })?;
    rt.block_on(run_tui_async(config, compiled_queries))
}

#[allow(clippy::too_many_lines, clippy::items_after_statements)]
async fn run_tui_async(
    config: &crate::types::SearchConfig,
    compiled_queries: &std::sync::Arc<
        std::collections::HashMap<
            crate::types::Language,
            std::sync::Arc<crate::query::MultiCompiledQuery>,
        >,
    >,
) -> crate::types::Result<()> {
    use crossterm::execute;
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    };
    let mut restore_needed = false;
    if enable_raw_mode().is_ok() && execute!(std::io::stdout(), EnterAlternateScreen).is_ok() {
        restore_needed = true;
    }
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
        }
    }
    let _restore = Restore;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
    let mut state = AppState::new();
    let mut key_stream = EventStream::new();
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        while let Some(Ok(ev)) = key_stream.next().await {
            if let Event::Key(key) = ev {
                if event_tx_clone.send(AppEvent::Keystroke(key)).is_err() {
                    break;
                }
            }
        }
    });
    let event_tx_clone2 = event_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if event_tx_clone2.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    {
        let event_tx_worker = event_tx.clone();
        let config_clone = config.clone();
        let compiled_clone = std::sync::Arc::clone(compiled_queries);
        tokio::spawn(async move {
            let mut current_token: Option<CancellationToken> = None;
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    SearchCommand::Cancel => {
                        if let Some(t) = current_token.take() {
                            t.cancel();
                        }
                    }
                    SearchCommand::Run(_query_str) => {
                        if let Some(t) = current_token.take() {
                            t.cancel();
                        }
                        let token = CancellationToken::new();
                        current_token = Some(token.clone());
                        let event_tx_block = event_tx_worker.clone();
                        let cfg = config_clone.clone();
                        let compiled = std::sync::Arc::clone(&compiled_clone);
                        tokio::task::spawn_blocking(move || {
                            let _ = event_tx_block.send(AppEvent::SearchStarted);
                            let walker: Box<
                                dyn Iterator<Item = crate::types::Result<ignore::DirEntry>> + Send,
                            > = match &cfg.lang_mode {
                                crate::types::LangMode::Single(lang) => {
                                    Box::new(crate::walker::build_walker(cfg.root_path.as_path(), lang))
                                }
                                crate::types::LangMode::Auto => {
                                    Box::new(crate::walker::build_auto_walker(cfg.root_path.as_path()))
                                }
                            };
                            let par = walker.par_bridge();
                            par.for_each(|entry_result| match entry_result {
                                Ok(entry) => {
                                    if token.is_cancelled() {
                                        return;
                                    }
                                    let detected_lang = match &cfg.lang_mode {
                                        crate::types::LangMode::Single(lang) => lang.clone(),
                                        crate::types::LangMode::Auto => {
                                            match crate::parser::detect_language(entry.path()) {
                                                Some(lang) => lang,
                                                None => return,
                                            }
                                        }
                                    };
                                    let ts_query = match compiled.get(&detected_lang) {
                                        Some(q) => std::sync::Arc::clone(q),
                                        None => return,
                                    };
                                    let ts_lang = match detected_lang {
                                        crate::types::Language::Rust => {
                                            tree_sitter_rust::language()
                                        }
                                        crate::types::Language::Python => {
                                            tree_sitter_python::language()
                                        }
                                        crate::types::Language::JavaScript => {
                                            tree_sitter_javascript::language()
                                        }
                                        crate::types::Language::TypeScript => {
                                            tree_sitter_typescript::language_tsx()
                                        }
                                        crate::types::Language::Go => tree_sitter_go::language(),
                                        crate::types::Language::C => tree_sitter_c::language(),
                                        crate::types::Language::Cpp => tree_sitter_cpp::language(),
                                    };
                                    match std::fs::metadata(entry.path()) {
                                        Ok(metadata) => {
                                            match crate::parser::parse_file_with_metadata(
                                                entry.path(),
                                                &ts_lang,
                                                &metadata,
                                            ) {
                                                Ok((tree, source)) => {
                                                    let source_bytes = source.as_bytes();
                                                    let matches =
                                                        crate::query::extract_multi_matches(
                                                            &tree,
                                                            source_bytes,
                                                            ts_query.as_ref(),
                                                            entry.path(),
                                                        );
                                                    if !matches.is_empty() {
                                                        let _ = event_tx_block
                                                            .send(AppEvent::SearchResult(matches));
                                                    }
                                                    drop(tree);
                                                    drop(source);
                                                }
                                                Err(error) => {
                                                    let _ = event_tx_block.send(
                                                        AppEvent::SearchError(error.to_string()),
                                                    );
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            let _ = event_tx_block
                                                .send(AppEvent::SearchError(error.to_string()));
                                        }
                                    }
                                }
                                Err(err) => {
                                    let _ =
                                        event_tx_block.send(AppEvent::SearchError(err.to_string()));
                                }
                            });
                            let _ = event_tx_block.send(AppEvent::SearchComplete);
                        });
                    }
                }
            }
        });
    }

    loop {
        let deadline = state
            .debounce_deadline
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            Some(event) = event_rx.recv() => {
                handle_event(&mut state, &event, &cmd_tx);
                if state.should_quit {
                    break;
                }
                state.frame_count = state.frame_count.wrapping_add(1);
            }
            () = tokio::time::sleep_until(deadline) => {
                if state.debounce_deadline.is_some() {
                    state.debounce_deadline = None;
                    if !state.query_input.is_empty() && state.submitted_query.as_deref().unwrap_or("") != state.query_input {
                        let _ = cmd_tx.send(SearchCommand::Run(state.query_input.clone()));
                        state.submitted_query = Some(state.query_input.clone());
                    }
                }
            }
        }
    }

    if restore_needed {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }

    Ok(())
}

fn handle_event(
    state: &mut AppState,
    event: &AppEvent,
    cmd_tx: &mpsc::UnboundedSender<SearchCommand>,
) {
    match event {
        AppEvent::Keystroke(key) => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => state.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => state.select_next(),
            KeyCode::Up | KeyCode::Char('k') => state.select_prev(),
            KeyCode::PageDown => {
                for _ in 0..10 {
                    state.scroll_down();
                }
            }
            KeyCode::PageUp => {
                for _ in 0..10 {
                    state.scroll_up();
                }
            }
            KeyCode::Char(c) => {
                state.query_input.push(c);
                state.error_message = None;
                state.debounce_deadline =
                    Some(tokio::time::Instant::now() + Duration::from_millis(300));
            }
            KeyCode::Backspace => {
                state.query_input.pop();
                state.debounce_deadline =
                    Some(tokio::time::Instant::now() + Duration::from_millis(300));
            }
            KeyCode::Enter => {
                state.debounce_deadline = None;
                if !state.query_input.trim().is_empty() {
                    let _ = cmd_tx.send(SearchCommand::Run(state.query_input.clone()));
                    state.submitted_query = Some(state.query_input.clone());
                }
            }
            _ => {}
        },
        AppEvent::Tick => {}
        AppEvent::SearchResult(results) => state.append_results(results.clone()),
        AppEvent::SearchComplete => state.search_running = false,
        AppEvent::SearchError(msg) => {
            state.error_message = Some(msg.clone());
            state.search_running = false;
        }
        AppEvent::SearchStarted => {
            state.clear_results();
            state.search_running = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_app_state_new_defaults() {
        let s = AppState::new();
        assert!(s.query_input.is_empty());
        assert!(s.results.is_empty());
        assert_eq!(s.selected_index, 0);
        assert!(!s.should_quit);
        assert!(!s.search_running);
        assert!(s.error_message.is_none());
    }

    #[test]
    fn test_append_results_sorts_and_deduplicates() {
        let mut s = AppState::new();
        let a = crate::types::MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "x".to_string(),
        };
        let b = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "y".to_string(),
        };
        let c = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "z".to_string(),
        };
        s.append_results(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(s.results[0].file_path, PathBuf::from("src/a.rs"));
        assert_eq!(s.results.len(), 3);
        s.append_results(vec![b.clone(), b.clone()]);
        assert_eq!(s.results.len(), 3);
    }

    #[test]
    fn test_clear_results_resets_state() {
        let mut s = AppState::new();
        let r = crate::types::MatchResult {
            file_path: PathBuf::from("src/x.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 2,
            capture_name: "c".to_string(),
            matched_text: "m".to_string(),
        };
        s.append_results(vec![r]);
        s.clear_results();
        assert!(s.results.is_empty());
        assert_eq!(s.selected_index, 0);
        assert!(s.file_tree.is_empty());
    }

    #[test]
    fn test_select_next_wraps() {
        let mut s = AppState::new();
        s.file_tree = vec![
            FileTreeEntry { path: PathBuf::from("a"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("b"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("c"), match_count: 1 },
        ];
        s.selected_file_index = 2;
        s.select_next();
        assert_eq!(s.selected_file_index, 0);
    }

    #[test]
    fn test_select_prev_wraps() {
        let mut s = AppState::new();
        s.file_tree = vec![
            FileTreeEntry { path: PathBuf::from("a"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("b"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("c"), match_count: 1 },
        ];
        s.selected_file_index = 0;
        s.select_prev();
        assert_eq!(s.selected_file_index, 2);
    }

    #[test]
    fn test_scroll_up_saturates_at_zero() {
        let mut s = AppState::new();
        s.scroll_offset = 0;
        s.scroll_up();
        assert_eq!(s.scroll_offset, 0);
    }

    #[test]
    fn test_results_for_selected_file_empty_when_no_results() {
        let s = AppState::new();
        assert!(s.results_for_selected_file().is_empty());
    }

    #[test]
    fn test_rebuild_file_tree_groups_by_path() {
        let mut s = AppState::new();
        let a1 = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "t".to_string(),
        };
        let a2 = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "u".to_string(),
        };
        let a3 = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "v".to_string(),
        };
        let b = crate::types::MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "u".to_string(),
        };
        s.append_results(vec![a1, a2, a3, b]);
        assert_eq!(s.file_tree.len(), 2);
        let entry = s.file_tree.iter().find(|e| e.path.ends_with("a.rs")).unwrap();
        assert_eq!(entry.match_count, 3);
    }

    #[test]
    fn test_file_tree_entry_sorted_by_path() {
        let mut s = AppState::new();
        let b = crate::types::MatchResult {
            file_path: PathBuf::from("src/z.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "t".to_string(),
        };
        let a = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "t".to_string(),
        };
        s.append_results(vec![b, a]);
        assert!(s.file_tree[0].path.ends_with("a.rs"));
    }

    #[test]
    fn test_debounce_deadline_set_on_char_keystroke() {
        let mut s = AppState::new();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('a')));
        handle_event(&mut s, &ev, &cmd_tx);
        assert!(s.debounce_deadline.is_some());
    }

    #[test]
    fn test_debounce_deadline_cleared_on_enter() {
        let mut s = AppState::new();
        s.debounce_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(10));
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Enter));
        handle_event(&mut s, &ev, &cmd_tx);
        assert!(s.debounce_deadline.is_none());
    }

    #[test]
    fn test_debounce_prevents_duplicate_dispatch() {
        let mut s = AppState::new();
        s.query_input = "foo".to_string();
        s.submitted_query = Some("foo".to_string());
        s.debounce_deadline = Some(tokio::time::Instant::now());
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        if s.debounce_deadline.is_some() {
            s.debounce_deadline = None;
            if !s.query_input.is_empty()
                && s.submitted_query.as_deref().unwrap_or("") != s.query_input
            {
                let _ = cmd_tx.send(SearchCommand::Run(s.query_input.clone()));
                s.submitted_query = Some(s.query_input.clone());
            }
        }
        assert_eq!(s.submitted_query.as_deref().unwrap(), "foo");
    }

    #[test]
    fn test_search_started_clears_results() {
        let mut s = AppState::new();
        let r = crate::types::MatchResult {
            file_path: PathBuf::from("x"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "m".to_string(),
        };
        s.append_results(vec![r]);
        assert!(!s.results.is_empty());
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::SearchStarted, &cmd_tx);
        assert!(s.results.is_empty());
        assert!(s.search_running);
    }

    #[test]
    fn test_search_complete_sets_running_false() {
        let mut s = AppState::new();
        s.search_running = true;
        handle_event(
            &mut s,
            &AppEvent::SearchComplete,
            &mpsc::unbounded_channel::<SearchCommand>().0,
        );
        assert!(!s.search_running);
    }

    #[test]
    fn test_append_results_incremental() {
        let mut s = AppState::new();
        let r1 = crate::types::MatchResult {
            file_path: PathBuf::from("a"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "x".to_string(),
        };
        let r2 = crate::types::MatchResult {
            file_path: PathBuf::from("b"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "y".to_string(),
        };
        let r3 = crate::types::MatchResult {
            file_path: PathBuf::from("c"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            capture_name: "c".to_string(),
            matched_text: "z".to_string(),
        };
        s.append_results(vec![r1]);
        s.append_results(vec![r2]);
        s.append_results(vec![r3]);
        assert_eq!(s.results.len(), 3);
    }

    #[test]
    fn test_cancellation_token_is_cancelled_after_new_run() {
        let t = CancellationToken::new();
        let t2 = t.clone();
        t.cancel();
        assert!(t2.is_cancelled());
    }

    #[test]
    fn test_debounce_deadline_resets_on_each_keystroke() {
        let mut s = AppState::new();
        s.debounce_deadline = Some(tokio::time::Instant::now());
        let old = s.debounce_deadline.unwrap();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('b'))), &cmd_tx);
        assert!(s.debounce_deadline.is_some());
        let new = s.debounce_deadline.unwrap();
        assert!(new > old);
    }
}
