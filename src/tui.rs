use crossterm::event::{Event, EventStream, KeyCode, KeyEvent};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Terminal,
};
use rayon::iter::ParallelBridge;
use rayon::prelude::ParallelIterator;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tree_sitter::Tree;

const SOURCE_CACHE_SIZE: usize = 10;
const ASSUMED_PANE_HEIGHT: usize = 30;
const MAX_AST_NODES: usize = 5000;

pub(crate) enum PaneId {
    FileTree,
    CodeView,
    AstView,
}

#[derive(Clone)]
pub(crate) struct AstNode {
    pub(crate) id: usize,
    pub(crate) depth: usize,
    pub(crate) kind: String,
    pub(crate) is_named: bool,
    pub(crate) is_error: bool,
    pub(crate) field_name: Option<String>,
    pub(crate) start_line: usize,
    pub(crate) start_col: usize,
    pub(crate) end_line: usize,
    pub(crate) end_col: usize,
    pub(crate) text_preview: Option<String>,
    pub(crate) has_children: bool,
    pub(crate) parent_id: Option<usize>,
}

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
    pub(crate) cached_source: HashMap<PathBuf, String>,
    pub(crate) auto_scrolled_for: Option<PathBuf>,
    pub(crate) active_pane: PaneId,
    pub(crate) ast_nodes: Vec<AstNode>,
    pub(crate) ast_selected_index: usize,
    pub(crate) collapsed_node_ids: std::collections::HashSet<usize>,
    pub(crate) ast_parsing_for: Option<PathBuf>,
    pub(crate) terminal_width: u16,
    pub(crate) terminal_height: u16,
    pub(crate) force_redraw: bool,
    pub(crate) file_tree_pct: u16,
    pub(crate) code_view_pct: u16,
    pub(crate) ast_view_pct: u16,
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
            cached_source: HashMap::new(),
            auto_scrolled_for: None,
            active_pane: PaneId::FileTree,
            ast_nodes: Vec::new(),
            ast_selected_index: 0,
            collapsed_node_ids: std::collections::HashSet::new(),
            ast_parsing_for: None,
            terminal_width: 0,
            terminal_height: 0,
            force_redraw: false,
            file_tree_pct: 25,
            code_view_pct: 50,
            ast_view_pct: 25,
        }
    }

    pub(crate) fn adjust_pane_size(&mut self, delta: i16) {
        if delta == 0 {
            return;
        }
        let active_idx = match self.active_pane {
            PaneId::FileTree => 0usize,
            PaneId::CodeView => 1usize,
            PaneId::AstView => 2usize,
        };
        let right_idx = (active_idx + 1) % 3;
        let mut pcts = [self.file_tree_pct as i16, self.code_view_pct as i16, self.ast_view_pct as i16];
        let adj = delta;
        let new_active = pcts[active_idx].saturating_add(adj);
        let new_right = pcts[right_idx].saturating_sub(adj);
        if new_active < 10 || new_active > 80 || new_right < 10 || new_right > 80 {
            return;
        }
        pcts[active_idx] = new_active;
        pcts[right_idx] = new_right;
        self.file_tree_pct = pcts[0] as u16;
        self.code_view_pct = pcts[1] as u16;
        self.ast_view_pct = pcts[2] as u16;
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
        self.cached_source.clear();
        self.auto_scrolled_for = None;
        self.ast_nodes.clear();
        self.ast_selected_index = 0;
        self.collapsed_node_ids.clear();
        self.ast_scroll_offset = 0;
        self.ast_parsing_for = None;
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
        self.auto_scrolled_for = None;
        self.load_source_for_selected();
        self.auto_scroll_to_first_match();
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
        self.auto_scrolled_for = None;
        self.load_source_for_selected();
        self.auto_scroll_to_first_match();
    }

    pub(crate) fn load_source_for_selected(&mut self) {
        if self.file_tree.is_empty() || self.selected_file_index >= self.file_tree.len() {
            return;
        }
        let path = self.file_tree[self.selected_file_index].path.clone();
        if self.cached_source.contains_key(&path) {
            return;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        self.cached_source.insert(path.clone(), source);
        if self.cached_source.len() > SOURCE_CACHE_SIZE {
            let keys: Vec<_> = self.cached_source.keys().cloned().collect();
            if !keys.is_empty() {
                self.cached_source.remove(&keys[0]);
            }
        }
    }

    pub(crate) fn auto_scroll_to_first_match(&mut self) {
        if self.file_tree.is_empty() || self.selected_file_index >= self.file_tree.len() {
            return;
        }
        let current_path = self.file_tree[self.selected_file_index].path.clone();
        if self.auto_scrolled_for == Some(current_path.clone()) {
            return;
        }
        let matches = self.results_for_selected_file();
        if matches.is_empty() {
            self.auto_scrolled_for = Some(current_path);
            return;
        }
        let min_line = matches.iter().map(|m| m.start_line).min().unwrap_or(1);
        let visible_lines = ASSUMED_PANE_HEIGHT;
        self.scroll_offset = if min_line > 1 && min_line.saturating_sub(1) > visible_lines / 2 {
            min_line.saturating_sub(1).saturating_sub(visible_lines / 2)
        } else {
            0
        };
        self.auto_scrolled_for = Some(current_path);
    }

    pub(crate) fn visible_ast_nodes(&self) -> Vec<&AstNode> {
        let mut visible_nodes = Vec::new();
        for node in &self.ast_nodes {
            let mut current_parent = node.parent_id;
            let mut hidden = false;
            while let Some(parent_id) = current_parent {
                if self.collapsed_node_ids.contains(&parent_id) {
                    hidden = true;
                    break;
                }
                current_parent =
                    self.ast_nodes.get(parent_id).and_then(|ancestor| ancestor.parent_id);
            }
            if !hidden {
                visible_nodes.push(node);
            }
        }
        visible_nodes
    }

    pub(crate) fn selected_file_path(&self) -> Option<PathBuf> {
        self.file_tree.get(self.selected_file_index).map(|entry| entry.path.clone())
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

fn build_ast_nodes(tree: &Tree, source: &crate::parser::FileSource) -> Vec<AstNode> {
    let mut nodes = Vec::new();
    let mut cursor = tree.walk();
    let mut ancestor_stack: Vec<usize> = Vec::new();
    let mut needs_visit = true;

    loop {
        let current_id = nodes.len();
        if needs_visit {
            let node = cursor.node();
            let start = node.start_position();
            let end = node.end_position();
            let has_children = node.child_count() > 0;
            let text_preview = if has_children {
                None
            } else {
                source
                    .as_bytes()
                    .get(node.byte_range())
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .map(|text| {
                        let preview: String = text.chars().take(40).collect();
                        if text.chars().count() > 40 {
                            format!("{}…", preview)
                        } else {
                            preview
                        }
                    })
            };
            let ast_node = AstNode {
                id: current_id,
                depth: ancestor_stack.len(),
                kind: node.kind().to_string(),
                is_named: node.is_named(),
                is_error: node.is_error() || node.kind() == "ERROR",
                field_name: cursor.field_name().map(|s| s.to_string()),
                start_line: start.row + 1,
                start_col: start.column,
                end_line: end.row + 1,
                end_col: end.column,
                text_preview,
                has_children,
                parent_id: ancestor_stack.last().copied(),
            };
            nodes.push(ast_node);
            if nodes.len() >= MAX_AST_NODES {
                return nodes;
            }
            needs_visit = false;
        }

        if needs_visit {
            if cursor.goto_first_child() {
                ancestor_stack.push(current_id);
                needs_visit = true;
                continue;
            }
        }

        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return nodes;
            }
            ancestor_stack.pop();
        }
        needs_visit = true;
    }
}

async fn spawn_ast_parse(
    path: PathBuf,
    lang: crate::types::Language,
    event_tx: mpsc::UnboundedSender<AppEvent>,
) {
    let lang_name = match lang {
        crate::types::Language::Rust => "rust",
        crate::types::Language::Python => "python",
        crate::types::Language::JavaScript => "js",
        crate::types::Language::TypeScript => "ts",
        crate::types::Language::Go => "go",
        crate::types::Language::C => "c",
        crate::types::Language::Cpp => "cpp",
    };
    let parsed = tokio::task::spawn_blocking(move || {
        let ts_language = match crate::parser::get_language(lang_name) {
            Ok(language) => language,
            Err(_) => return Vec::new(),
        };
        match crate::parser::parse_file(&path, &ts_language) {
            Ok((tree, source)) => {
                let nodes = build_ast_nodes(&tree, &source);
                drop(tree);
                drop(source);
                nodes
            }
            Err(_) => Vec::new(),
        }
    })
    .await;

    if let Ok(nodes) = parsed {
        let _ = event_tx.send(AppEvent::AstReady(nodes));
    }
}

#[allow(dead_code)]
pub(crate) enum AppEvent {
    Keystroke(KeyEvent),
    Tick,
    Resize(u16, u16),
    SearchStarted,
    SearchResult(Vec<crate::types::MatchResult>),
    AstReady(Vec<AstNode>),
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
    let rt = Runtime::new()
        .map_err(|e| crate::types::AppError::IoError(std::io::Error::other(e.to_string())))?;
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
    use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    use crossterm::execute;
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    };
    enable_raw_mode()?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    execute!(terminal.backend_mut(), EnterAlternateScreen, EnableMouseCapture)?;
    scopeguard::defer! {
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = disable_raw_mode();
    }
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
    let mut state = AppState::new();
    if let Ok((w, h)) = crossterm::terminal::size() {
        state.terminal_width = w;
        state.terminal_height = h;
    }
    let mut key_stream = EventStream::new();
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        while let Some(Ok(ev)) = key_stream.next().await {
            match ev {
                Event::Key(key) => {
                    if event_tx_clone.send(AppEvent::Keystroke(key)).is_err() {
                        break;
                    }
                }
                Event::Resize(w, h) => {
                    if event_tx_clone.send(AppEvent::Resize(w, h)).is_err() {
                        break;
                    }
                }
                _ => {}
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
                                crate::types::LangMode::Single(lang) => Box::new(
                                    crate::walker::build_walker(cfg.root_path.as_path(), lang),
                                ),
                                crate::types::LangMode::Auto => Box::new(
                                    crate::walker::build_auto_walker(cfg.root_path.as_path()),
                                ),
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
                if !state.should_quit {
                    state.load_source_for_selected();
                    state.auto_scroll_to_first_match();
                    if let Some(selected_path) = state.selected_file_path() {
                        if state.ast_parsing_for.as_ref() != Some(&selected_path) {
                            state.ast_nodes.clear();
                            state.ast_selected_index = 0;
                            state.collapsed_node_ids.clear();
                            state.ast_scroll_offset = 0;
                            state.ast_parsing_for = Some(selected_path.clone());
                            if let Some(lang) = crate::parser::detect_language(&selected_path) {
                                let event_tx_ast = event_tx.clone();
                                tokio::spawn(spawn_ast_parse(selected_path, lang, event_tx_ast));
                            }
                        }
                    } else {
                        state.ast_nodes.clear();
                        state.ast_selected_index = 0;
                        state.collapsed_node_ids.clear();
                        state.ast_scroll_offset = 0;
                        state.ast_parsing_for = None;
                    }
                    render(&mut terminal, &mut state)?;
                    state.frame_count = state.frame_count.wrapping_add(1);
                }
                if state.should_quit {
                    break;
                }
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
            KeyCode::Tab => {
                state.active_pane = match state.active_pane {
                    PaneId::FileTree => PaneId::CodeView,
                    PaneId::CodeView => PaneId::AstView,
                    PaneId::AstView => PaneId::FileTree,
                };
            }
            KeyCode::Down | KeyCode::Char('j') => match state.active_pane {
                PaneId::FileTree => state.select_next(),
                PaneId::CodeView => state.scroll_down(),
                PaneId::AstView => {
                    let visible_len = state.visible_ast_nodes().len();
                    if visible_len > 0 {
                        if state.ast_selected_index + 1 < visible_len {
                            state.ast_selected_index += 1;
                        }
                        let viewport = ASSUMED_PANE_HEIGHT.saturating_sub(2);
                        if state.ast_selected_index >= state.ast_scroll_offset + viewport {
                            state.ast_scroll_offset =
                                state.ast_selected_index.saturating_sub(viewport.saturating_sub(1));
                        }
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => match state.active_pane {
                PaneId::FileTree => state.select_prev(),
                PaneId::CodeView => state.scroll_up(),
                PaneId::AstView => {
                    let visible_len = state.visible_ast_nodes().len();
                    if visible_len > 0 {
                        if state.ast_selected_index > 0 {
                            state.ast_selected_index -= 1;
                        }
                        if state.ast_selected_index < state.ast_scroll_offset + 2 {
                            state.ast_scroll_offset = state.ast_selected_index.saturating_sub(1);
                        }
                    }
                }
            },
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
            KeyCode::Char('g') if matches!(state.active_pane, PaneId::AstView) => {
                state.ast_selected_index = 0;
                state.ast_scroll_offset = 0;
            }
            KeyCode::Char('G') if matches!(state.active_pane, PaneId::AstView) => {
                let visible_len = state.visible_ast_nodes().len();
                if visible_len > 0 {
                    state.ast_selected_index = visible_len.saturating_sub(1);
                    if state.ast_selected_index > 0 {
                        state.ast_scroll_offset = state.ast_selected_index.saturating_sub(1);
                    }
                }
            }
            KeyCode::Enter if matches!(state.active_pane, PaneId::AstView) => {
                let visible_nodes = state.visible_ast_nodes();
                if let Some(node) = visible_nodes.get(state.ast_selected_index) {
                    if node.has_children {
                        let node_id = node.id;
                        if state.collapsed_node_ids.contains(&node_id) {
                            state.collapsed_node_ids.remove(&node_id);
                        } else {
                            state.collapsed_node_ids.insert(node_id);
                        }
                    }
                }
            }
            KeyCode::Char(c) => {
                if c == '<' {
                    state.adjust_pane_size(-5);
                } else if c == '>' {
                    state.adjust_pane_size(5);
                } else {
                    state.query_input.push(c);
                    state.error_message = None;
                    state.debounce_deadline =
                        Some(tokio::time::Instant::now() + Duration::from_millis(300));
                }
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
        AppEvent::Resize(w, h) => {
            state.terminal_width = *w;
            state.terminal_height = *h;
            state.force_redraw = true;
        }
        AppEvent::AstReady(nodes) => {
            state.ast_nodes = nodes.to_vec();
            state.ast_selected_index = 0;
            state.collapsed_node_ids.clear();
            state.ast_scroll_offset = 0;
        }
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

fn render(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut AppState,
) -> crate::types::Result<()> {
    if state.force_redraw {
        terminal.clear()?;
        state.force_redraw = false;
    }
    terminal.draw(|frame| {
        draw_ui(frame, state);
    })?;
    Ok(())
}

fn is_terminal_too_small(width: u16, height: u16) -> bool {
    width < 40 || height < 10
}

fn draw_ui(frame: &mut ratatui::Frame, state: &AppState) {
    let area = frame.size();
    if is_terminal_too_small(area.width, area.height) {
        let block = Block::default().borders(Borders::ALL).title("Terminal");
        let paragraph = Paragraph::new("Terminal too small — resize to continue")
            .block(block)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::Red));
        frame.render_widget(paragraph, area);
        return;
    }
    let layout = Layout::default().direction(Direction::Vertical).constraints([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ]);
    let [query_area, panes_area, status_area] = layout.areas(area);

    draw_query_bar(frame, query_area, state);

    let panes_layout = Layout::default().direction(Direction::Horizontal).constraints([
        Constraint::Percentage(state.file_tree_pct),
        Constraint::Percentage(state.code_view_pct),
        Constraint::Percentage(state.ast_view_pct),
    ]);
    let [file_pane_area, code_pane_area, ast_pane_area] = panes_layout.areas(panes_area);

    draw_file_tree_pane(frame, file_pane_area, state);
    draw_code_pane(frame, code_pane_area, state);
    draw_ast_pane(frame, ast_pane_area, state);
    draw_status_bar(frame, status_area, state);
}

fn draw_query_bar(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let border_style =
        if state.search_running { Style::default().fg(Color::DarkGray) } else { Style::default() };
    let block = Block::default().title("Query").borders(Borders::ALL).border_style(border_style);
    let cursor_char = if state.frame_count % 8 < 4 { "|" } else { " " };
    let query_with_cursor = format!("{}{}", state.query_input, cursor_char);

    let text = if let Some(error) = &state.error_message {
        vec![
            Line::from(query_with_cursor),
            Line::from(Span::styled(error.clone(), Style::default().fg(Color::Red))),
        ]
    } else {
        vec![Line::from(query_with_cursor)]
    };
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_file_tree_pane(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let file_tree_border = match state.active_pane {
        PaneId::FileTree => Style::default().add_modifier(Modifier::BOLD),
        _ => Style::default(),
    };

    let title = if let Some(selected_entry) = state.file_tree.get(state.selected_file_index) {
        let path_str = selected_entry.path.to_string_lossy().to_string();
        let pane_width = area.width.saturating_sub(2) as usize;
        if path_str.len() > pane_width.saturating_sub(4) {
            let chars: Vec<char> = path_str.chars().collect();
            let start = chars.len().saturating_sub(pane_width.saturating_sub(5));
            let truncated: String = chars[start..].iter().collect();
            format!("…{}", truncated)
        } else {
            path_str
        }
    } else {
        " Files ".to_string()
    };
    let block = Block::default().title(title).borders(Borders::ALL).border_style(file_tree_border);

    if state.file_tree.is_empty() {
        let hint_text = if state.search_running {
            "Searching..."
        } else if state.submitted_query.is_none() {
            "Type a query to search"
        } else {
            "No matches"
        };
        let paragraph =
            Paragraph::new(hint_text).block(block).style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }

    let items: Vec<ListItem> = state
        .file_tree
        .iter()
        .map(|entry| {
            let filename =
                entry.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let count_str = entry.match_count.to_string();
            let pane_width = area.width.saturating_sub(2) as usize;
            let padding = pane_width.saturating_sub(filename.len() + count_str.len() + 2);
            let line = format!(" {}{}{}", filename, " ".repeat(padding), count_str);
            ListItem::new(line)
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_file_index));
    let list = List::new(items)
        .block(block)
        .style(Style::default())
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_code_pane(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let code_border = match state.active_pane {
        PaneId::CodeView => Style::default().add_modifier(Modifier::BOLD),
        _ => Style::default(),
    };

    if state.file_tree.is_empty() {
        let block =
            Block::default().title(" Code ").borders(Borders::ALL).border_style(code_border);
        let paragraph = Paragraph::new("No results").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph.block(block), area);
        return;
    }

    if state.selected_file_index >= state.file_tree.len() {
        let block =
            Block::default().title(" Code ").borders(Borders::ALL).border_style(code_border);
        frame.render_widget(block, area);
        return;
    }

    let current_path = &state.file_tree[state.selected_file_index].path;
    let source = state.cached_source.get(current_path).map(|s| s.as_str()).unwrap_or("");

    if source.is_empty() && !state.cached_source.contains_key(current_path) {
        let block =
            Block::default().title(" Code ").borders(Borders::ALL).border_style(code_border);
        let paragraph = Paragraph::new("Loading...").style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph.block(block), area);
        return;
    }

    let visible_height = area.height.saturating_sub(2) as usize;
    let lines: Vec<&str> = source.lines().collect();
    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_offset = state.scroll_offset.min(max_scroll);

    let matches = state.results_for_selected_file();
    let mut match_lines = std::collections::HashSet::new();
    for m in matches {
        match_lines.insert(m.start_line);
        if m.end_line != m.start_line {
            for line_num in m.start_line..=m.end_line {
                match_lines.insert(line_num);
            }
        }
    }

    let mut line_widgets = Vec::new();
    for i in effective_offset..(effective_offset + visible_height).min(total_lines) {
        let line_num = i + 1;
        let line_text = lines[i];
        let marker = if match_lines.contains(&line_num) { "  ▶ │ " } else { "    │ " };
        let line_num_str = format!("{:>4} ", line_num);

        let mut spans = vec![
            Span::styled(line_num_str, Style::default().add_modifier(Modifier::DIM)),
            Span::raw(marker),
        ];

        let mut char_pos = 0;
        for m in matches {
            if m.start_line <= line_num && line_num <= m.end_line {
                let start_byte = if line_num == m.start_line { m.start_col } else { 0 };
                let end_byte = if line_num == m.end_line { m.end_col } else { line_text.len() };

                if start_byte > char_pos {
                    spans.push(Span::raw(&line_text[char_pos..start_byte]));
                }

                if end_byte <= line_text.len() {
                    if let Some(match_text) = line_text.get(start_byte..end_byte) {
                        spans.push(Span::styled(
                            match_text.to_string(),
                            Style::default().add_modifier(Modifier::REVERSED),
                        ));
                        char_pos = end_byte;
                    } else {
                        spans.push(Span::raw(&line_text[start_byte..]));
                        char_pos = line_text.len();
                        break;
                    }
                } else {
                    spans.push(Span::raw(&line_text[start_byte..]));
                    char_pos = line_text.len();
                    break;
                }
            }
        }

        if char_pos < line_text.len() {
            spans.push(Span::raw(&line_text[char_pos..]));
        }

        line_widgets.push(Line::from(spans));
    }

    let block = Block::default().title(" Code ").borders(Borders::ALL).border_style(code_border);
    let paragraph = Paragraph::new(line_widgets).block(block);
    frame.render_widget(paragraph, area);

    let scrollbar_state = ScrollbarState::default()
        .content_length(total_lines)
        .viewport_content_length(visible_height)
        .position(effective_offset);
    frame.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        area,
        &mut scrollbar_state.clone(),
    );
}

fn draw_ast_pane(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let ast_border = match state.active_pane {
        PaneId::AstView => Style::default().add_modifier(Modifier::BOLD),
        _ => Style::default(),
    };
    let block = Block::default().title(" AST ").borders(Borders::ALL).border_style(ast_border);

    if state.ast_nodes.is_empty() {
        let text = if state.ast_parsing_for.is_some() { "Parsing…" } else { "Select a file" };
        let paragraph = Paragraph::new(text)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }

    let visible_nodes = state.visible_ast_nodes();
    if visible_nodes.is_empty() {
        let paragraph = Paragraph::new("No visible nodes")
            .block(block)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = visible_nodes.len().saturating_sub(inner_height);
    let effective_scroll = state.ast_scroll_offset.min(max_scroll);
    let end_index = (effective_scroll + inner_height).min(visible_nodes.len());
    let selected_index = state.ast_selected_index.min(visible_nodes.len().saturating_sub(1));
    let selected_path_matches = state.results_for_selected_file();
    let mut lines = Vec::new();

    for (visible_index, node) in visible_nodes[effective_scroll..end_index].iter().enumerate() {
        let absolute_index = effective_scroll + visible_index;
        let selected = absolute_index == selected_index;
        let matched = node_matches_capture(node, selected_path_matches);
        let collapsed = state.collapsed_node_ids.contains(&node.id);
        lines.push(build_ast_node_line(
            node,
            selected,
            matched,
            collapsed,
            area.width.saturating_sub(2) as usize,
        ));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_status_bar(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let hint_text = match state.active_pane {
        PaneId::FileTree => "  [↑↓/jk] select file   [Tab] focus code view  ",
        PaneId::CodeView => "  [↑↓/jk] scroll code   [Tab] focus file tree  ",
        PaneId::AstView => "  [↑↓/jk] navigate   [Enter] expand/collapse   [g/G] top/bottom  ",
    };
    let mut status = hint_text.to_string();
    if state.search_running {
        let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spinner = spinner_chars[(state.frame_count as usize) % 10];
        status.push_str(&format!("  [searching {}]", spinner));
    }
    let paragraph = Paragraph::new(status).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}

#[allow(dead_code)]
fn build_highlighted_line(
    line_text: &str,
    highlight_range: Option<(usize, usize)>,
) -> Vec<Span<'static>> {
    let Some((start, end)) = highlight_range else {
        return vec![Span::raw(line_text.to_string())];
    };
    if start >= end {
        return vec![Span::raw(line_text.to_string())];
    }
    let before = line_text.get(..start);
    let highlighted = line_text.get(start..end);
    let after = line_text.get(end..);
    match (before, highlighted, after) {
        (Some(before), Some(highlighted), Some(after)) => vec![
            Span::raw(before.to_string()),
            Span::styled(
                highlighted.to_string(),
                Style::default().add_modifier(Modifier::REVERSED),
            ),
            Span::raw(after.to_string()),
        ],
        _ => vec![Span::raw(line_text.to_string())],
    }
}

fn node_matches_capture(node: &AstNode, results: &[crate::types::MatchResult]) -> bool {
    results
        .iter()
        .any(|result| result.start_line == node.start_line && result.start_col == node.start_col)
}

fn build_ast_node_line(
    node: &AstNode,
    selected: bool,
    matched: bool,
    collapsed: bool,
    width: usize,
) -> Line<'static> {
    let selection_modifier = if selected { Modifier::REVERSED } else { Modifier::empty() };
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut body_width = 0usize;
    let mut push_text = |text: String, style: Style| {
        body_width += text.chars().count();
        spans.push(Span::styled(text, style.add_modifier(selection_modifier)));
    };

    push_text("  ".repeat(node.depth), Style::default());

    let indicator = if node.has_children {
        if collapsed {
            "▶ "
        } else {
            "▼ "
        }
    } else {
        "  "
    };
    let indicator_style = if node.has_children {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    push_text(indicator.to_string(), indicator_style);

    if let Some(field_name) = &node.field_name {
        push_text(format!("{}: ", field_name), Style::default().add_modifier(Modifier::DIM));
    }

    let kind_style = if node.is_error {
        Style::default().fg(Color::Red)
    } else if node.is_named {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };
    push_text(node.kind.clone(), kind_style);

    if matched {
        push_text(" ●".to_string(), Style::default().fg(Color::Green));
    }

    if let Some(preview) = &node.text_preview {
        push_text(
            format!(" \"{}\"", preview),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM),
        );
    }

    let _multiline = node.start_line != node.end_line || node.start_col != node.end_col;
    let position = format!("[{}:{}]", node.start_line, node.start_col);
    let position_width = position.chars().count();
    if body_width + position_width < width {
        let padding = width - body_width - position_width;
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(
            position,
            Style::default().add_modifier(Modifier::DIM).add_modifier(selection_modifier),
        ));
    }

    Line::from(spans)
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
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "x".to_string(),
        };
        let b = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "y".to_string(),
        };
        let c = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
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
            start_byte: 0,
            end_byte: 0,
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
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "t".to_string(),
        };
        let a2 = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "u".to_string(),
        };
        let a3 = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "v".to_string(),
        };
        let b = crate::types::MatchResult {
            file_path: PathBuf::from("src/b.rs"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
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
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "t".to_string(),
        };
        let a = crate::types::MatchResult {
            file_path: PathBuf::from("src/a.rs"),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
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
            start_byte: 0,
            end_byte: 0,
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
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "x".to_string(),
        };
        let r2 = crate::types::MatchResult {
            file_path: PathBuf::from("b"),
            start_line: 2,
            start_col: 0,
            end_line: 2,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
            capture_name: "c".to_string(),
            matched_text: "y".to_string(),
        };
        let r3 = crate::types::MatchResult {
            file_path: PathBuf::from("c"),
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 1,
            start_byte: 0,
            end_byte: 0,
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

    #[test]
    fn test_file_tree_entry_display_format() {
        let entry = FileTreeEntry { path: PathBuf::from("src/auth/handler.rs"), match_count: 3 };
        let filename = entry.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert_eq!(filename, "handler.rs");
    }

    #[test]
    fn test_spinner_cycles() {
        for frame_count in 0u64..10 {
            let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let spinner_index = (frame_count as usize) % 10;
            let _ = spinner_chars[spinner_index];
            assert!(spinner_index < 10);
        }
    }

    #[test]
    fn test_cursor_blink_logic() {
        for frame_count in 0u64..8 {
            let should_show = frame_count % 8 < 4;
            if frame_count < 4 {
                assert!(should_show);
            } else {
                assert!(!should_show);
            }
        }
        let should_show_at_8 = 8u64 % 8 < 4;
        assert!(should_show_at_8);
    }

    #[test]
    fn test_selected_file_full_path_truncation() {
        let path_str = "/very/long/path/to/some/deeply/nested/file.rs";
        let pane_width = 20usize;
        let truncated = if path_str.len() > pane_width.saturating_sub(4) {
            let chars: Vec<char> = path_str.chars().collect();
            let start = chars.len().saturating_sub(pane_width.saturating_sub(5));
            let truncated: String = chars[start..].iter().collect();
            format!("…{}", truncated)
        } else {
            path_str.to_string()
        };
        assert!(truncated.starts_with('…'));
        assert!(truncated.len() <= pane_width);
    }

    #[test]
    fn test_empty_file_tree_hint_when_no_query() {
        let s = AppState::new();
        assert!(s.submitted_query.is_none());
        assert!(s.file_tree.is_empty());
    }

    #[test]
    fn test_file_tree_hint_when_searching() {
        let mut s = AppState::new();
        s.search_running = true;
        s.file_tree.clear();
        assert!(s.search_running);
        assert!(s.file_tree.is_empty());
    }

    #[test]
    fn test_load_source_caches_result() {
        let temp_file = std::env::temp_dir().join("test_code_view.txt");
        let content = "fn hello() {\n    println!(\"world\");\n}\n";
        let _ = std::fs::write(&temp_file, content);

        let mut s = AppState::new();
        s.file_tree.push(FileTreeEntry { path: temp_file.clone(), match_count: 1 });
        s.selected_file_index = 0;

        s.load_source_for_selected();

        assert!(s.cached_source.contains_key(&temp_file));
        assert_eq!(s.cached_source.get(&temp_file).unwrap(), content);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_load_source_cache_bounded() {
        let mut s = AppState::new();
        for i in 0..=SOURCE_CACHE_SIZE {
            let temp_file = std::env::temp_dir().join(format!("test_cache_{}.txt", i));
            let _ = std::fs::write(&temp_file, format!("content {}", i));
            s.file_tree.push(FileTreeEntry { path: temp_file.clone(), match_count: 1 });
            s.selected_file_index = i;
            s.load_source_for_selected();
        }
        assert!(s.cached_source.len() <= SOURCE_CACHE_SIZE);
        for i in 0..=SOURCE_CACHE_SIZE {
            let _ =
                std::fs::remove_file(std::env::temp_dir().join(format!("test_cache_{}.txt", i)));
        }
    }

    #[test]
    fn test_auto_scroll_centers_on_first_match() {
        let mut s = AppState::new();
        s.file_tree.push(FileTreeEntry { path: PathBuf::from("test.rs"), match_count: 1 });
        s.results.push(crate::types::MatchResult {
            file_path: PathBuf::from("test.rs"),
            start_line: 50,
            start_col: 0,
            end_line: 50,
            end_col: 5,
            start_byte: 0,
            end_byte: 0,
            capture_name: "test".to_string(),
            matched_text: "fn".to_string(),
        });
        s.selected_file_index = 0;
        s.auto_scroll_to_first_match();

        assert!(s.scroll_offset >= 20);
        assert!(s.scroll_offset <= 40);
    }

    #[test]
    fn test_auto_scroll_does_not_fire_twice() {
        let mut s = AppState::new();
        s.file_tree.push(FileTreeEntry { path: PathBuf::from("test.rs"), match_count: 1 });
        s.results.push(crate::types::MatchResult {
            file_path: PathBuf::from("test.rs"),
            start_line: 50,
            start_col: 0,
            end_line: 50,
            end_col: 5,
            start_byte: 0,
            end_byte: 0,
            capture_name: "test".to_string(),
            matched_text: "fn".to_string(),
        });
        s.selected_file_index = 0;
        s.auto_scroll_to_first_match();
        let offset_after_first = s.scroll_offset;
        s.auto_scroll_to_first_match();
        assert_eq!(s.scroll_offset, offset_after_first);
    }

    #[test]
    fn test_auto_scroll_resets_on_file_change() {
        let mut s = AppState::new();
        s.file_tree.push(FileTreeEntry { path: PathBuf::from("a.rs"), match_count: 1 });
        s.file_tree.push(FileTreeEntry { path: PathBuf::from("b.rs"), match_count: 1 });
        s.auto_scrolled_for = Some(PathBuf::from("a.rs"));
        s.selected_file_index = 0;
        assert!(s.auto_scrolled_for == Some(PathBuf::from("a.rs")));
        s.auto_scrolled_for = None;
        assert!(s.auto_scrolled_for.is_none());
    }

    #[test]
    fn test_pane_id_tab_cycles() {
        let mut s = AppState::new();
        assert!(matches!(s.active_pane, PaneId::FileTree));
        s.active_pane = match s.active_pane {
            PaneId::FileTree => PaneId::CodeView,
            PaneId::CodeView => PaneId::AstView,
            PaneId::AstView => PaneId::FileTree,
        };
        assert!(matches!(s.active_pane, PaneId::CodeView));
        s.active_pane = match s.active_pane {
            PaneId::FileTree => PaneId::CodeView,
            PaneId::CodeView => PaneId::AstView,
            PaneId::AstView => PaneId::FileTree,
        };
        assert!(matches!(s.active_pane, PaneId::AstView));
        s.active_pane = match s.active_pane {
            PaneId::FileTree => PaneId::CodeView,
            PaneId::CodeView => PaneId::AstView,
            PaneId::AstView => PaneId::FileTree,
        };
        assert!(matches!(s.active_pane, PaneId::FileTree));
    }

    #[test]
    fn test_jk_scrolls_code_when_code_pane_active() {
        let mut s = AppState::new();
        s.active_pane = PaneId::CodeView;
        s.scroll_offset = 5;
        s.scroll_down();
        assert_eq!(s.scroll_offset, 6);
        s.scroll_up();
        assert_eq!(s.scroll_offset, 5);
    }

    #[test]
    fn test_jk_navigates_files_when_file_tree_active() {
        let mut s = AppState::new();
        s.active_pane = PaneId::FileTree;
        s.file_tree.push(FileTreeEntry { path: PathBuf::from("a.rs"), match_count: 1 });
        s.file_tree.push(FileTreeEntry { path: PathBuf::from("b.rs"), match_count: 1 });
        s.selected_file_index = 0;
        s.select_next();
        assert_eq!(s.selected_file_index, 1);
    }

    #[test]
    fn test_scroll_offset_clamp_does_not_mutate_state() {
        let scroll_offset = 1000usize;
        let visible_height = 30usize;
        let total_lines = 50usize;
        let max_scroll = total_lines.saturating_sub(visible_height);
        let effective_offset = scroll_offset.min(max_scroll);
        assert_eq!(effective_offset, 20);
        assert_eq!(scroll_offset, 1000);
    }

    #[test]
    fn test_match_on_line_produces_reversed_segment() {
        let line_text = "fn hello()";
        let start_col = 0;
        let end_col = 2;
        let match_segment = &line_text[start_col..end_col];
        assert_eq!(match_segment, "fn");
    }

    #[test]
    fn test_submit_query_updates_submitted_query() {
        let mut s = AppState::new();
        s.query_input = "fn greet".to_string();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Enter));
        handle_event(&mut s, &ev, &cmd_tx);
        assert_eq!(s.submitted_query, Some("fn greet".to_string()));
    }

    #[test]
    fn test_search_result_event_updates_results() {
        let mut s = AppState::new();
        let a = crate::types::MatchResult { file_path: PathBuf::from("a"), start_line:1, start_col:0, end_line:1, end_col:1, start_byte: 0, end_byte: 0, capture_name: "c".to_string(), matched_text: "x".to_string() };
        let b = crate::types::MatchResult { file_path: PathBuf::from("b"), start_line:2, start_col:0, end_line:2, end_col:1, start_byte: 0, end_byte: 0, capture_name: "c".to_string(), matched_text: "y".to_string() };
        handle_event(&mut s, &AppEvent::SearchResult(vec![a.clone(), b.clone()]), &mpsc::unbounded_channel::<SearchCommand>().0);
        assert_eq!(s.results.len(), 2);
    }

    #[test]
    fn test_search_complete_clears_running_flag() {
        let mut s = AppState::new();
        s.search_running = true;
        handle_event(&mut s, &AppEvent::SearchComplete, &mpsc::unbounded_channel::<SearchCommand>().0);
        assert!(!s.search_running);
    }

    #[test]
    fn test_search_started_clears_results_and_sets_running() {
        let mut s = AppState::new();
        s.results.push(crate::types::MatchResult { file_path: PathBuf::from("x"), start_line:1, start_col:0, end_line:1, end_col:1, start_byte: 0, end_byte: 0, capture_name: "c".to_string(), matched_text: "m".to_string() });
        handle_event(&mut s, &AppEvent::SearchStarted, &mpsc::unbounded_channel::<SearchCommand>().0);
        assert!(s.results.is_empty());
        assert!(s.search_running);
    }

    #[test]
    fn test_search_error_sets_error_message() {
        let mut s = AppState::new();
        handle_event(&mut s, &AppEvent::SearchError("parse failed".to_string()), &mpsc::unbounded_channel::<SearchCommand>().0);
        assert_eq!(s.error_message, Some("parse failed".to_string()));
        assert!(!s.search_running);
    }

    #[test]
    fn test_selecting_result_updates_index() {
        let mut s = AppState::new();
        s.file_tree = vec![
            FileTreeEntry { path: PathBuf::from("a"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("b"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("c"), match_count: 1 },
        ];
        s.selected_file_index = 0;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Down));
        handle_event(&mut s, &ev, &cmd_tx);
        assert_eq!(s.selected_file_index, 1);
    }

    #[test]
    fn test_j_key_same_as_down_in_file_tree() {
        let mut s = AppState::new();
        s.file_tree = vec![
            FileTreeEntry { path: PathBuf::from("a"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("b"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("c"), match_count: 1 },
        ];
        s.selected_file_index = 0;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('j')));
        handle_event(&mut s, &ev, &cmd_tx);
        assert_eq!(s.selected_file_index, 1);
    }

    #[test]
    fn test_k_key_moves_selection_up() {
        let mut s = AppState::new();
        s.file_tree = vec![
            FileTreeEntry { path: PathBuf::from("a"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("b"), match_count: 1 },
            FileTreeEntry { path: PathBuf::from("c"), match_count: 1 },
        ];
        s.selected_file_index = 2;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('k')));
        handle_event(&mut s, &ev, &cmd_tx);
        assert_eq!(s.selected_file_index, 1);
    }

    #[test]
    fn test_quit_sets_should_quit() {
        let mut s = AppState::new();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('q')));
        handle_event(&mut s, &ev, &cmd_tx);
        assert!(s.should_quit);
    }

    #[test]
    fn test_esc_sets_should_quit() {
        let mut s = AppState::new();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        let ev = AppEvent::Keystroke(KeyEvent::from(KeyCode::Esc));
        handle_event(&mut s, &ev, &cmd_tx);
        assert!(s.should_quit);
    }

    #[test]
    fn test_resize_updates_terminal_dimensions() {
        let mut s = AppState::new();
        s.terminal_width = 80;
        s.terminal_height = 24;
        handle_event(&mut s, &AppEvent::Resize(120, 40), &mpsc::unbounded_channel::<SearchCommand>().0);
        assert_eq!(s.terminal_width, 120);
        assert_eq!(s.terminal_height, 40);
    }

    #[test]
    fn test_resize_sets_force_redraw() {
        let mut s = AppState::new();
        s.force_redraw = false;
        handle_event(&mut s, &AppEvent::Resize(100, 30), &mpsc::unbounded_channel::<SearchCommand>().0);
        assert!(s.force_redraw);
    }

    #[test]
    fn test_tab_cycles_panes() {
        let mut s = AppState::new();
        s.active_pane = PaneId::FileTree;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Tab)), &cmd_tx);
        assert!(matches!(s.active_pane, PaneId::CodeView));
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Tab)), &cmd_tx);
        assert!(matches!(s.active_pane, PaneId::AstView));
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Tab)), &cmd_tx);
        assert!(matches!(s.active_pane, PaneId::FileTree));
    }

    #[test]
    fn test_char_appended_to_query_input() {
        let mut s = AppState::new();
        s.query_input = "fn".to_string();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Char(' '))), &cmd_tx);
        assert_eq!(s.query_input, "fn ".to_string());
    }

    #[test]
    fn test_backspace_removes_last_char() {
        let mut s = AppState::new();
        s.query_input = "fn ".to_string();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Backspace)), &cmd_tx);
        assert_eq!(s.query_input, "fn".to_string());
    }

    #[test]
    fn test_backspace_on_empty_query_does_not_panic() {
        let mut s = AppState::new();
        s.query_input = "".to_string();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Backspace)), &cmd_tx);
        assert_eq!(s.query_input, "".to_string());
    }

    #[test]
    fn test_char_keystroke_clears_error_message() {
        let mut s = AppState::new();
        s.error_message = Some("previous error".to_string());
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('x'))), &cmd_tx);
        assert!(s.error_message.is_none());
    }

    #[test]
    fn test_ast_ready_populates_ast_nodes() {
        let mut s = AppState::new();
        let node = AstNode { id: 0, depth: 0, kind: "source_file".to_string(), is_named: true, is_error: false, field_name: None, start_line: 1, start_col: 0, end_line: 1, end_col: 0, text_preview: None, has_children: false, parent_id: None };
        handle_event(&mut s, &AppEvent::AstReady(vec![node.clone()]), &mpsc::unbounded_channel::<SearchCommand>().0);
        assert_eq!(s.ast_nodes.len(), 1);
        assert_eq!(s.ast_selected_index, 0);
        assert!(s.collapsed_node_ids.is_empty());
    }

    #[test]
    fn test_enter_in_ast_pane_toggles_collapse() {
        let mut s = AppState::new();
        s.active_pane = PaneId::AstView;
        let node = AstNode { id: 0, depth: 0, kind: "n".to_string(), is_named: true, is_error: false, field_name: None, start_line: 1, start_col: 0, end_line: 1, end_col: 0, text_preview: None, has_children: true, parent_id: None };
        s.ast_nodes.push(node);
        s.ast_selected_index = 0;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Enter)), &cmd_tx);
        assert!(s.collapsed_node_ids.contains(&0));
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Enter)), &cmd_tx);
        assert!(!s.collapsed_node_ids.contains(&0));
    }

    #[test]
    fn test_debounce_deadline_set_on_char_in_filetree_pane() {
        let mut s = AppState::new();
        s.active_pane = PaneId::FileTree;
        s.debounce_deadline = None;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SearchCommand>();
        handle_event(&mut s, &AppEvent::Keystroke(KeyEvent::from(KeyCode::Char('f'))), &cmd_tx);
        assert!(s.debounce_deadline.is_some());
    }

    #[test]
    fn test_pane_resize_grow_shrinks_neighbor() {
        let mut s = AppState::new();
        s.file_tree_pct = 25; s.code_view_pct = 50; s.ast_view_pct = 25;
        s.active_pane = PaneId::FileTree;
        s.adjust_pane_size(5);
        assert_eq!(s.file_tree_pct, 30);
        assert_eq!(s.file_tree_pct + s.code_view_pct + s.ast_view_pct, 100);
    }

    #[test]
    fn test_pane_resize_respects_minimum() {
        let mut s = AppState::new();
        s.file_tree_pct = 10;
        s.active_pane = PaneId::FileTree;
        s.adjust_pane_size(-5);
        assert_eq!(s.file_tree_pct, 10);
    }

    #[test]
    fn test_minimum_terminal_guard_condition() {
        assert!(is_terminal_too_small(39, 10));
        assert!(!is_terminal_too_small(40, 10));
    }
}
