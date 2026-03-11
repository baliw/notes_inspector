use crate::export;
use crate::obsidian;
use crate::tree::{self, FlatItem, TreeNode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum NoteSource {
    AppleNotes,
    Obsidian,
}

#[derive(Debug)]
pub enum Screen {
    /// Initial source selection screen.
    SourceSelect {
        selected: usize,
        options: Vec<(&'static str, NoteSource)>,
        error_message: Option<String>,
    },
    /// Folder browser for selecting an Obsidian vault.
    FolderSelect {
        current_path: PathBuf,
        entries: Vec<PathBuf>,
        selected: usize,
        scroll_offset: usize,
        message: Option<String>,
    },
    /// Main notes browser with tree + preview.
    NotesBrowser {
        source: NoteSource,
        tree_roots: Vec<TreeNode>,
        flat_items: Vec<FlatItem>,
        selected: usize,
        tree_scroll: usize,
        note_content: Option<String>,
        note_scroll: usize,
        /// Stats about the notes database.
        stats: NoteStats,
        /// Focus: true = tree, false = note preview.
        focus_tree: bool,
        /// Attachment analysis popup.
        attachment_popup: Option<AttachmentPopup>,
        /// Error message to display.
        error_message: Option<String>,
    },
    /// Export destination picker: discovered vaults + browse/create options.
    ExportVaultSelect {
        /// None = still scanning, Some = scan complete.
        vaults: Option<Vec<PathBuf>>,
        /// Shared handle for async scan results.
        scan_handle: std::sync::Arc<std::sync::Mutex<Option<Vec<PathBuf>>>>,
        selected: usize,
        scroll_offset: usize,
        folder_filter: Option<Vec<i64>>,
        /// When Some, user is typing a new vault name.
        new_name_input: Option<String>,
    },
    /// Folder browser for selecting export output directory.
    ExportFolderSelect {
        current_path: PathBuf,
        entries: Vec<PathBuf>,
        selected: usize,
        scroll_offset: usize,
        /// If set, only export these folder PKs (sub-tree export).
        folder_filter: Option<Vec<i64>>,
        /// When Some, user is typing a new directory name.
        new_name_input: Option<String>,
    },
    /// Export results / log viewer (live progress via shared log).
    ExportResults {
        shared_log: export::SharedExportLog,
        scroll: usize,
    },
}

#[derive(Debug, Clone)]
pub struct NoteStats {
    pub total_notes: usize,
    pub total_folders: usize,
    pub total_attachments: usize,
    pub vault_name: String,
}

#[derive(Debug)]
pub struct AttachmentPopup {
    pub analysis: obsidian::AttachmentAnalysis,
    pub selected: usize,
    pub scroll: usize,
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            screen: Screen::SourceSelect {
                selected: 0,
                options: vec![
                    ("Apple Notes", NoteSource::AppleNotes),
                    ("Obsidian", NoteSource::Obsidian),
                ],
                error_message: None,
            },
            should_quit: false,
        }
    }

    /// Returns true if an async operation is running (for poll-based redraw).
    pub fn is_exporting(&self) -> bool {
        match &self.screen {
            Screen::ExportResults { shared_log, .. } => {
                !shared_log.lock().unwrap().is_complete
            }
            Screen::ExportVaultSelect { vaults: None, .. } => true, // scanning
            _ => false,
        }
    }

    /// Update async state: auto-scroll export log, check vault scan completion.
    pub fn update_export_progress(&mut self) {
        match &mut self.screen {
            Screen::ExportResults { shared_log, scroll, .. } => {
                let log = shared_log.lock().unwrap();
                if !log.is_complete {
                    *scroll = log.lines.len().saturating_sub(1);
                }
            }
            Screen::ExportVaultSelect {
                vaults,
                scan_handle,
                ..
            } => {
                if vaults.is_none() {
                    let mut handle = scan_handle.lock().unwrap();
                    if handle.is_some() {
                        *vaults = handle.take();
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(dead_code)]
    pub fn can_quit(&self) -> bool {
        // Allow 'q' to quit unless we're in a text input or popup
        !matches!(
            &self.screen,
            Screen::NotesBrowser {
                attachment_popup: Some(_),
                ..
            }
        )
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Global quit with Ctrl+C
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // Determine screen type without borrowing mutably
        let screen_type = match &self.screen {
            Screen::SourceSelect { .. } => 0,
            Screen::FolderSelect { .. } => 1,
            Screen::NotesBrowser { .. } => 2,
            Screen::ExportVaultSelect { .. } => 3,
            Screen::ExportFolderSelect { .. } => 4,
            Screen::ExportResults { .. } => 5,
        };

        match screen_type {
            0 => self.handle_source_select(key),
            1 => self.handle_folder_select(key),
            2 => self.handle_notes_browser(key),
            3 => self.handle_export_vault_select(key),
            4 => self.handle_export_folder_select(key),
            5 => self.handle_export_results(key),
            _ => {}
        }
    }

    fn handle_source_select(&mut self, key: KeyEvent) {
        let Screen::SourceSelect {
            ref mut selected,
            ref options,
            ref mut error_message,
        } = self.screen
        else {
            return;
        };

        // Clear error on navigation
        *error_message = None;

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected < options.len() - 1 {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                let source = options[*selected].1.clone();
                match source {
                    NoteSource::Obsidian => {
                        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                        let entries = obsidian::list_subdirs(&home);
                        self.screen = Screen::FolderSelect {
                            current_path: home,
                            entries,
                            selected: 0,
                            scroll_offset: 0,
                            message: None,
                        };
                    }
                    NoteSource::AppleNotes => match crate::apple::build_notes_tree() {
                        Ok(tree) => {
                            let stats = NoteStats {
                                total_notes: tree.count_notes(),
                                total_folders: tree.count_folders(),
                                total_attachments: 0,
                                vault_name: "Apple Notes".to_string(),
                            };
                            let flat = tree::flatten_tree(&[tree.clone()]);
                            self.screen = Screen::NotesBrowser {
                                source: NoteSource::AppleNotes,
                                tree_roots: vec![tree],
                                flat_items: flat,
                                selected: 0,
                                tree_scroll: 0,
                                note_content: None,
                                note_scroll: 0,
                                stats,
                                focus_tree: true,
                                attachment_popup: None,
                                error_message: None,
                            };
                        }
                        Err(e) => {
                            *error_message = Some(e);
                        }
                    },
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            _ => {}
        }
    }

    fn handle_folder_select(&mut self, key: KeyEvent) {
        // Extract fields we need - take ownership of screen temporarily
        let Screen::FolderSelect {
            ref mut current_path,
            ref mut entries,
            ref mut selected,
            ref mut scroll_offset,
            ref mut message,
        } = self.screen
        else {
            return;
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                    if *selected < *scroll_offset {
                        *scroll_offset = *selected;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected < entries.len().saturating_sub(1) {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(path) = entries.get(*selected).cloned() {
                    if obsidian::is_obsidian_vault(&path) {
                        // Open the vault
                        let tree = obsidian::build_vault_tree(&path);
                        let stats = NoteStats {
                            total_notes: tree.count_notes(),
                            total_folders: tree.count_folders(),
                            total_attachments: tree.count_attachments(),
                            vault_name: tree.name.clone(),
                        };
                        let flat = tree::flatten_tree(&[tree.clone()]);
                        self.screen = Screen::NotesBrowser {
                            source: NoteSource::Obsidian,
                            tree_roots: vec![tree],
                            flat_items: flat,
                            selected: 0,
                            tree_scroll: 0,
                            note_content: None,
                            note_scroll: 0,
                            stats,
                            focus_tree: true,
                            attachment_popup: None,
                            error_message: None,
                        };
                        return;
                    } else {
                        // Navigate into folder
                        *current_path = path;
                        *entries = obsidian::list_subdirs(current_path);
                        *selected = 0;
                        *scroll_offset = 0;
                        *message = if entries.is_empty() {
                            Some("No subdirectories found".to_string())
                        } else {
                            None
                        };
                    }
                }
            }
            KeyCode::Backspace | KeyCode::Left => {
                // Go to parent directory
                if let Some(parent) = current_path.parent() {
                    let parent = parent.to_path_buf();
                    *entries = obsidian::list_subdirs(&parent);
                    *current_path = parent;
                    *selected = 0;
                    *scroll_offset = 0;
                    *message = None;
                }
            }
            KeyCode::Esc => {
                // Go back to source select
                self.screen = Screen::SourceSelect {
                    selected: 0,
                    options: vec![
                        ("Apple Notes", NoteSource::AppleNotes),
                        ("Obsidian", NoteSource::Obsidian),
                    ],
                    error_message: None,
                };
            }
            _ => {}
        }
    }

    fn handle_notes_browser(&mut self, key: KeyEvent) {
        let Screen::NotesBrowser {
            ref source,
            ref mut tree_roots,
            ref mut flat_items,
            ref mut selected,
            ref mut tree_scroll,
            ref mut note_content,
            ref mut note_scroll,
            ref mut focus_tree,
            ref mut attachment_popup,
            ref mut error_message,
            ..
        } = self.screen
        else {
            return;
        };

        // Handle attachment popup
        if let Some(popup) = attachment_popup {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    *attachment_popup = None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if popup.selected > 0 {
                        popup.selected -= 1;
                        if popup.selected < popup.scroll {
                            popup.scroll = popup.selected;
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if popup.selected < popup.analysis.unlinked.len().saturating_sub(1) {
                        popup.selected += 1;
                    }
                }
                _ => {}
            }
            return;
        }

        // Clear error message on any key
        *error_message = None;

        match key.code {
            KeyCode::Tab => {
                *focus_tree = !*focus_tree;
            }
            KeyCode::Esc => {
                self.screen = Screen::SourceSelect {
                    selected: 0,
                    options: vec![
                        ("Apple Notes", NoteSource::AppleNotes),
                        ("Obsidian", NoteSource::Obsidian),
                    ],
                    error_message: None,
                };
                return;
            }
            _ => {}
        }

        if *focus_tree {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if *selected > 0 {
                        *selected -= 1;
                        // Skip over divider lines
                        if flat_items.get(*selected).is_some_and(|i| i.kind == tree::NodeKind::Divider) && *selected > 0 {
                            *selected -= 1;
                        }
                        // Scrolling is handled by the UI based on viewport height
                        load_selected_note(source, tree_roots, flat_items, *selected, note_content);
                        *note_scroll = 0;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *selected < flat_items.len().saturating_sub(1) {
                        *selected += 1;
                        // Skip over divider lines
                        if flat_items.get(*selected).is_some_and(|i| i.kind == tree::NodeKind::Divider)
                            && *selected < flat_items.len().saturating_sub(1)
                        {
                            *selected += 1;
                        }
                        // Scrolling is handled by the UI based on viewport height
                        load_selected_note(source, tree_roots, flat_items, *selected, note_content);
                        *note_scroll = 0;
                    }
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    if let Some(item) = flat_items.get(*selected) {
                        if item.has_children {
                            // Toggle expand/collapse
                            if let Some(node) =
                                tree::get_node_mut(tree_roots, &item.index_path)
                            {
                                node.expanded = !node.expanded;
                            }
                            *flat_items = tree::flatten_tree(tree_roots);
                        } else {
                            // Load note content
                            load_selected_note(
                                source,
                                tree_roots,
                                flat_items,
                                *selected,
                                note_content,
                            );
                            *note_scroll = 0;
                            *focus_tree = false;
                        }
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    if let Some(item) = flat_items.get(*selected) {
                        if item.expanded {
                            // Collapse
                            if let Some(node) =
                                tree::get_node_mut(tree_roots, &item.index_path)
                            {
                                node.expanded = false;
                            }
                            *flat_items = tree::flatten_tree(tree_roots);
                        } else if item.index_path.len() > 1 {
                            // Go to parent
                            let parent_path =
                                &item.index_path[..item.index_path.len() - 1];
                            if let Some(parent_idx) = flat_items
                                .iter()
                                .position(|fi| fi.index_path == parent_path)
                            {
                                *selected = parent_idx;
                                if *selected < *tree_scroll {
                                    *tree_scroll = *selected;
                                }
                            }
                        }
                    }
                }
                KeyCode::Char('a') if *source == NoteSource::Obsidian => {
                    // Open attachment analysis
                    if let Some(root) = tree_roots.first() {
                        let analysis =
                            obsidian::analyze_attachments(&root.path);
                        *attachment_popup = Some(AttachmentPopup {
                            analysis,
                            selected: 0,
                            scroll: 0,
                        });
                    }
                }
                KeyCode::Char('e') if *source == NoteSource::AppleNotes => {
                    // Export Apple Notes to Obsidian
                    // If a non-root item is selected, export only that sub-tree
                    let folder_filter = if let Some(item) = flat_items.get(*selected) {
                        if item.index_path.len() <= 1 {
                            None
                        } else {
                            if let Some(node) = tree::get_node(tree_roots, &item.index_path) {
                                let pks = collect_folder_pks(node);
                                if pks.is_empty() {
                                    if item.index_path.len() >= 2 {
                                        let parent_path = &item.index_path[..item.index_path.len() - 1];
                                        if let Some(parent) = tree::get_node(tree_roots, parent_path) {
                                            let parent_pks = collect_folder_pks(parent);
                                            if parent_pks.is_empty() { None } else { Some(parent_pks) }
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    Some(pks)
                                }
                            } else {
                                None
                            }
                        }
                    } else {
                        None
                    };

                    // Scan for existing Obsidian vaults asynchronously
                    let scan_handle = std::sync::Arc::new(std::sync::Mutex::new(None));
                    let handle_clone = scan_handle.clone();
                    std::thread::spawn(move || {
                        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                        let found = obsidian::find_vaults(&home, 4);
                        *handle_clone.lock().unwrap() = Some(found);
                    });
                    self.screen = Screen::ExportVaultSelect {
                        vaults: None,
                        scan_handle,
                        selected: 0,
                        scroll_offset: 0,
                        folder_filter,
                        new_name_input: None,
                    };
                    return;
                }
                KeyCode::Char('d') if *source == NoteSource::AppleNotes => {
                    // Debug: copy note attachment info to clipboard
                    if let Some(item) = flat_items.get(*selected) {
                        if item.kind == tree::NodeKind::Note {
                            if let Some(node) = tree::get_node(tree_roots, &item.index_path) {
                                let path_str = node.path.to_string_lossy().to_string();
                                let debug_info = crate::apple::debug_note(&path_str);
                                // Copy to clipboard via pbcopy
                                use std::io::Write;
                                if let Ok(mut child) = std::process::Command::new("pbcopy")
                                    .stdin(std::process::Stdio::piped())
                                    .spawn()
                                {
                                    if let Some(mut stdin) = child.stdin.take() {
                                        let _ = stdin.write_all(debug_info.as_bytes());
                                        // stdin is dropped here, closing the pipe so pbcopy sees EOF
                                    }
                                    let _ = child.wait();
                                    *error_message = Some("Debug info copied to clipboard".to_string());
                                } else {
                                    *error_message = Some("Failed to run pbcopy".to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        } else {
            // Note preview scrolling
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *note_scroll = note_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *note_scroll += 1;
                }
                KeyCode::PageUp => {
                    *note_scroll = note_scroll.saturating_sub(20);
                }
                KeyCode::PageDown => {
                    *note_scroll += 20;
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    *focus_tree = true;
                }
                KeyCode::Char('d') if *source == NoteSource::AppleNotes => {
                    // Debug: copy note attachment info to clipboard
                    if let Some(item) = flat_items.get(*selected) {
                        if item.kind == tree::NodeKind::Note {
                            if let Some(node) = tree::get_node(tree_roots, &item.index_path) {
                                let path_str = node.path.to_string_lossy().to_string();
                                let debug_info = crate::apple::debug_note(&path_str);
                                use std::io::Write;
                                if let Ok(mut child) = std::process::Command::new("pbcopy")
                                    .stdin(std::process::Stdio::piped())
                                    .spawn()
                                {
                                    if let Some(mut stdin) = child.stdin.take() {
                                        let _ = stdin.write_all(debug_info.as_bytes());
                                    }
                                    let _ = child.wait();
                                    *error_message = Some("Debug info copied to clipboard".to_string());
                                } else {
                                    *error_message = Some("Failed to run pbcopy".to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_export_vault_select(&mut self, key: KeyEvent) {
        let Screen::ExportVaultSelect {
            ref vaults,
            ref mut selected,
            ref mut scroll_offset,
            ref folder_filter,
            ref mut new_name_input,
            ..
        } = self.screen
        else {
            return;
        };

        // Still scanning — only allow Esc
        let vault_list = match vaults {
            Some(v) => v,
            None => {
                if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                    self.screen = Screen::SourceSelect {
                        selected: 0,
                        options: vec![
                            ("Apple Notes", NoteSource::AppleNotes),
                            ("Obsidian", NoteSource::Obsidian),
                        ],
                        error_message: None,
                    };
                }
                return;
            }
        };

        // Text input mode for new vault name
        if let Some(input) = new_name_input {
            match key.code {
                KeyCode::Esc => {
                    *new_name_input = None;
                }
                KeyCode::Enter => {
                    let name = input.trim().to_string();
                    if !name.is_empty() {
                        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                        let vault_path = home.join(&name);
                        if std::fs::create_dir_all(&vault_path).is_err() {
                            *new_name_input = None;
                            return;
                        }
                        let _ = obsidian::init_vault(&vault_path);
                        let filter = folder_filter.clone();
                        let config = export::ExportConfig {
                            output_dir: vault_path,
                            attachments_folder: "_attachments".to_string(),
                            folder_filter: filter,
                        };
                        let shared_log = export::run_export_async(config);
                        self.screen = Screen::ExportResults { shared_log, scroll: 0 };
                    } else {
                        *new_name_input = None;
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => {
                    if !matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
                        && !c.is_control()
                    {
                        input.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        // The list is: [vaults...] [divider] ["Browse folders..."] ["Create new vault..."]
        let vault_count = vault_list.len();
        let browse_idx = vault_count; // index after divider
        let create_idx = vault_count + 1;
        let total_items = vault_count + 2; // browse + create

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                    if *selected < *scroll_offset {
                        *scroll_offset = *selected;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected < total_items.saturating_sub(1) {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if *selected < vault_count {
                    // Selected a vault — export to it
                    let vault_path = vault_list[*selected].clone();
                    let filter = folder_filter.clone();
                    let config = export::ExportConfig {
                        output_dir: vault_path,
                        attachments_folder: "_attachments".to_string(),
                        folder_filter: filter,
                    };
                    let shared_log = export::run_export_async(config);
                    self.screen = Screen::ExportResults { shared_log, scroll: 0 };
                } else if *selected == browse_idx {
                    // Browse folders manually
                    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                    let entries = obsidian::list_subdirs(&home);
                    let filter = folder_filter.clone();
                    self.screen = Screen::ExportFolderSelect {
                        current_path: home,
                        entries,
                        selected: 0,
                        scroll_offset: 0,
                        folder_filter: filter,
                        new_name_input: None,
                    };
                } else if *selected == create_idx {
                    // Enter text input mode for new vault name
                    *new_name_input = Some(String::new());
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                // Go back — rebuild Apple Notes browser
                match crate::apple::build_notes_tree() {
                    Ok(tree) => {
                        let stats = NoteStats {
                            total_notes: tree.count_notes(),
                            total_folders: tree.count_folders(),
                            total_attachments: 0,
                            vault_name: "Apple Notes".to_string(),
                        };
                        let flat = tree::flatten_tree(&[tree.clone()]);
                        self.screen = Screen::NotesBrowser {
                            source: NoteSource::AppleNotes,
                            tree_roots: vec![tree],
                            flat_items: flat,
                            selected: 0,
                            tree_scroll: 0,
                            note_content: None,
                            note_scroll: 0,
                            stats,
                            focus_tree: true,
                            attachment_popup: None,
                            error_message: None,
                        };
                    }
                    Err(_) => {
                        self.screen = Screen::SourceSelect {
                            selected: 0,
                            options: vec![
                                ("Apple Notes", NoteSource::AppleNotes),
                                ("Obsidian", NoteSource::Obsidian),
                            ],
                            error_message: None,
                        };
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_export_folder_select(&mut self, key: KeyEvent) {
        let Screen::ExportFolderSelect {
            ref mut current_path,
            ref mut entries,
            ref mut selected,
            ref mut scroll_offset,
            ref folder_filter,
            ref mut new_name_input,
        } = self.screen
        else {
            return;
        };

        // Text input mode for new directory name
        if let Some(input) = new_name_input {
            match key.code {
                KeyCode::Esc => {
                    *new_name_input = None;
                }
                KeyCode::Enter => {
                    let name = input.trim().to_string();
                    if !name.is_empty() {
                        let vault_path = current_path.join(&name);
                        if std::fs::create_dir_all(&vault_path).is_err() {
                            *new_name_input = None;
                            return;
                        }
                        let _ = obsidian::init_vault(&vault_path);
                        let filter = folder_filter.clone();
                        let config = export::ExportConfig {
                            output_dir: vault_path,
                            attachments_folder: "_attachments".to_string(),
                            folder_filter: filter,
                        };
                        let shared_log = export::run_export_async(config);
                        self.screen = Screen::ExportResults { shared_log, scroll: 0 };
                    } else {
                        *new_name_input = None;
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => {
                    if !matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
                        && !c.is_control()
                    {
                        input.push(c);
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if *selected > 0 {
                    *selected -= 1;
                    if *selected < *scroll_offset {
                        *scroll_offset = *selected;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected < entries.len().saturating_sub(1) {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(path) = entries.get(*selected).cloned() {
                    // Navigate into folder
                    *current_path = path;
                    *entries = obsidian::list_subdirs(current_path);
                    *selected = 0;
                    *scroll_offset = 0;
                }
            }
            KeyCode::Backspace | KeyCode::Left => {
                if let Some(parent) = current_path.parent() {
                    let parent = parent.to_path_buf();
                    *entries = obsidian::list_subdirs(&parent);
                    *current_path = parent;
                    *selected = 0;
                    *scroll_offset = 0;
                }
            }
            KeyCode::Char('x') => {
                // Confirm export to current_path — initialize vault if needed
                let output_dir = current_path.clone();
                if !obsidian::is_obsidian_vault(&output_dir) {
                    let _ = obsidian::init_vault(&output_dir);
                }
                let filter = folder_filter.clone();
                let config = export::ExportConfig {
                    output_dir,
                    attachments_folder: "_attachments".to_string(),
                    folder_filter: filter,
                };
                let shared_log = export::run_export_async(config);
                self.screen = Screen::ExportResults { shared_log, scroll: 0 };
            }
            KeyCode::Char('n') => {
                // Enter text input mode to create a new subdirectory as a vault
                *new_name_input = Some(String::new());
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                // Go back — rebuild Apple Notes browser
                match crate::apple::build_notes_tree() {
                    Ok(tree) => {
                        let stats = NoteStats {
                            total_notes: tree.count_notes(),
                            total_folders: tree.count_folders(),
                            total_attachments: 0,
                            vault_name: "Apple Notes".to_string(),
                        };
                        let flat = tree::flatten_tree(&[tree.clone()]);
                        self.screen = Screen::NotesBrowser {
                            source: NoteSource::AppleNotes,
                            tree_roots: vec![tree],
                            flat_items: flat,
                            selected: 0,
                            tree_scroll: 0,
                            note_content: None,
                            note_scroll: 0,
                            stats,
                            focus_tree: true,
                            attachment_popup: None,
                            error_message: None,
                        };
                    }
                    Err(_) => {
                        self.screen = Screen::SourceSelect {
                            selected: 0,
                            options: vec![
                                ("Apple Notes", NoteSource::AppleNotes),
                                ("Obsidian", NoteSource::Obsidian),
                            ],
                            error_message: None,
                        };
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_export_results(&mut self, key: KeyEvent) {
        let Screen::ExportResults {
            ref shared_log,
            ref mut scroll,
        } = self.screen
        else {
            return;
        };

        let log = shared_log.lock().unwrap();
        let line_count = log.lines.len();
        let is_complete = log.is_complete;
        drop(log);

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                *scroll = scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *scroll < line_count.saturating_sub(1) {
                    *scroll += 1;
                }
            }
            KeyCode::PageUp => {
                *scroll = scroll.saturating_sub(20);
            }
            KeyCode::PageDown => {
                *scroll += 20;
                let max = line_count.saturating_sub(1);
                if *scroll > max {
                    *scroll = max;
                }
            }
            KeyCode::Esc | KeyCode::Char('q') if is_complete => {
                self.screen = Screen::SourceSelect {
                    selected: 0,
                    options: vec![
                        ("Apple Notes", NoteSource::AppleNotes),
                        ("Obsidian", NoteSource::Obsidian),
                    ],
                    error_message: None,
                };
            }
            _ => {}
        }
    }
}

fn load_selected_note(
    source: &NoteSource,
    tree_roots: &[TreeNode],
    flat_items: &[FlatItem],
    selected: usize,
    note_content: &mut Option<String>,
) {
    if let Some(item) = flat_items.get(selected) {
        match item.kind {
            tree::NodeKind::Note => {
                if let Some(node) = tree::get_node(tree_roots, &item.index_path) {
                    let content = match source {
                        NoteSource::Obsidian => obsidian::read_note(&node.path),
                        NoteSource::AppleNotes => {
                            crate::apple::read_note(&node.path.to_string_lossy())
                        }
                    };
                    *note_content = Some(content);
                }
            }
            tree::NodeKind::Attachment => {
                if let Some(node) = tree::get_node(tree_roots, &item.index_path) {
                    // For image attachments, show a placeholder indicating it's an image
                    let ext = node
                        .path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    let is_image =
                        ["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg"].contains(&ext.as_str());
                    if is_image {
                        // Mark content with a special prefix so the UI knows to render as image
                        *note_content =
                            Some(format!("__IMAGE__:{}", node.path.to_string_lossy()));
                    } else {
                        *note_content = Some(format!(
                            "Attachment: {}\nType: {ext}\nPath: {}",
                            item.name,
                            node.path.to_string_lossy()
                        ));
                    }
                }
            }
            tree::NodeKind::Folder | tree::NodeKind::Divider => {
                *note_content = None;
            }
        }
    }
}

/// Collect all folder PKs from a TreeNode subtree.
/// Extracts Z_PK from paths like "apple-notes://folder/{pk}".
fn collect_folder_pks(node: &TreeNode) -> Vec<i64> {
    let mut pks = Vec::new();
    collect_folder_pks_recursive(node, &mut pks);
    pks
}

fn collect_folder_pks_recursive(node: &TreeNode, pks: &mut Vec<i64>) {
    if node.kind == tree::NodeKind::Folder {
        let path_str = node.path.to_string_lossy();
        if let Some(pk_str) = path_str.strip_prefix("apple-notes://folder/") {
            if let Ok(pk) = pk_str.parse::<i64>() {
                pks.push(pk);
            }
        }
    }
    for child in &node.children {
        collect_folder_pks_recursive(child, pks);
    }
}
