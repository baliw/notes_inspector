//! Export Apple Notes to Obsidian-compatible Markdown.

use crate::apple;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Configuration for an export run.
pub struct ExportConfig {
    pub output_dir: PathBuf,
    pub attachments_folder: String,
    /// If set, only export notes in these folder PKs.
    pub folder_filter: Option<Vec<i64>>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("."),
            attachments_folder: "_attachments".to_string(),
            folder_filter: None,
        }
    }
}

/// Accumulated log and statistics from an export.
#[derive(Debug)]
pub struct ExportLog {
    pub lines: Vec<String>,
    pub notes_exported: usize,
    pub attachments_copied: usize,
    pub folders_created: usize,
    pub errors: usize,
    pub is_complete: bool,
}

impl ExportLog {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            notes_exported: 0,
            attachments_copied: 0,
            folders_created: 0,
            errors: 0,
            is_complete: false,
        }
    }

    fn log(&mut self, msg: impl Into<String>) {
        self.lines.push(msg.into());
    }

    fn error(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        self.lines.push(format!("ERROR: {s}"));
        self.errors += 1;
    }
}

/// Thread-safe shared export log for real-time progress display.
pub type SharedExportLog = std::sync::Arc<std::sync::Mutex<ExportLog>>;

/// Spawn the export in a background thread, returning a shared log handle.
pub fn run_export_async(config: ExportConfig) -> SharedExportLog {
    let shared_log = std::sync::Arc::new(std::sync::Mutex::new(ExportLog::new()));
    let log_handle = shared_log.clone();

    std::thread::spawn(move || {
        run_export_into(&config, &log_handle);
    });

    shared_log
}

// ============================================================================
// Folder hierarchy
// ============================================================================

struct FolderInfo {
    pk: i64,
    title: String,
    parent_pk: Option<i64>,
    children: Vec<i64>,
    /// Relative path from output root (e.g. "Work/Projects").
    rel_path: String,
}

fn build_folder_hierarchy(conn: &Connection) -> Result<Vec<FolderInfo>, String> {
    let has_parent = conn
        .prepare("SELECT ZPARENT FROM ZICCLOUDSYNCINGOBJECT LIMIT 0")
        .is_ok();

    let sql = if has_parent {
        "SELECT Z_PK, ZTITLE2, ZPARENT FROM ZICCLOUDSYNCINGOBJECT WHERE ZTITLE2 IS NOT NULL"
    } else {
        "SELECT Z_PK, ZTITLE2 FROM ZICCLOUDSYNCINGOBJECT WHERE ZTITLE2 IS NOT NULL"
    };

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut map: HashMap<i64, FolderInfo> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();

    let rows = stmt
        .query_map([], |row| {
            let pk: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let parent: Option<i64> = if has_parent { row.get(2)? } else { None };
            Ok((pk, title, parent))
        })
        .map_err(|e| e.to_string())?;

    for row in rows {
        let (pk, title, parent_pk) = row.map_err(|e| e.to_string())?;
        map.insert(
            pk,
            FolderInfo {
                pk,
                title,
                parent_pk,
                children: Vec::new(),
                rel_path: String::new(),
            },
        );
        order.push(pk);
    }

    // Build children lists
    let child_map: Vec<(i64, i64)> = map
        .values()
        .filter_map(|f| f.parent_pk.map(|ppk| (ppk, f.pk)))
        .collect();
    for (ppk, cpk) in child_map {
        if let Some(parent) = map.get_mut(&ppk) {
            parent.children.push(cpk);
        }
    }

    // Compute relative paths (BFS from roots)
    let root_pks: Vec<i64> = map
        .values()
        .filter(|f| f.parent_pk.is_none() || !map.contains_key(&f.parent_pk.unwrap_or(-1)))
        .map(|f| f.pk)
        .collect();

    let mut queue: std::collections::VecDeque<(i64, String)> = std::collections::VecDeque::new();
    for &rpk in &root_pks {
        if let Some(f) = map.get(&rpk) {
            let path = apple::sanitize_filename(&f.title);
            queue.push_back((rpk, path));
        }
    }
    while let Some((pk, path)) = queue.pop_front() {
        let children = if let Some(f) = map.get_mut(&pk) {
            f.rel_path = path.clone();
            f.children.clone()
        } else {
            continue;
        };
        for cpk in children {
            if let Some(child) = map.get(&cpk) {
                let child_path =
                    format!("{}/{}", path, apple::sanitize_filename(&child.title));
                queue.push_back((cpk, child_path));
            }
        }
    }

    // Return in original order
    let result: Vec<FolderInfo> = order.into_iter().filter_map(|pk| map.remove(&pk)).collect();
    Ok(result)
}

// ============================================================================
// Attachment cache
// ============================================================================

struct AttachmentEntry {
    file_path: Option<PathBuf>,
    filename: String,
    type_uti: String,
}

struct AttachmentCache {
    entries: HashMap<String, AttachmentEntry>,
}

fn build_attachment_cache(
    conn: &Connection,
    notes_base: &Path,
) -> AttachmentCache {
    let mut cache = AttachmentCache {
        entries: HashMap::new(),
    };

    // Follow up to 2 levels of ZMEDIA chain
    let sql = "\
        SELECT \
            a.ZIDENTIFIER, a.ZTYPEUTI, a.ZTITLE, a.ZFILENAME, a.ZMEDIA, \
            m.ZIDENTIFIER, m.ZFILENAME, m.ZMEDIA, \
            m2.ZIDENTIFIER, m2.ZFILENAME \
        FROM ZICCLOUDSYNCINGOBJECT a \
        LEFT JOIN ZICCLOUDSYNCINGOBJECT m ON a.ZMEDIA = m.Z_PK \
        LEFT JOIN ZICCLOUDSYNCINGOBJECT m2 ON m.ZMEDIA = m2.Z_PK \
        WHERE a.ZTYPEUTI IS NOT NULL";

    let Ok(mut stmt) = conn.prepare(sql) else {
        return cache;
    };

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,  // att_uuid
            row.get::<_, Option<String>>(1)?,  // type_uti
            row.get::<_, Option<String>>(2)?,  // att_title
            row.get::<_, Option<String>>(3)?,  // att_filename
            row.get::<_, Option<String>>(5)?,  // media_uuid
            row.get::<_, Option<String>>(6)?,  // media_filename
            row.get::<_, Option<String>>(8)?,  // media2_uuid
            row.get::<_, Option<String>>(9)?,  // media2_filename
        ))
    });

    let Ok(rows) = rows else { return cache };

    for row in rows.flatten() {
        let (att_uuid, type_uti, att_title, att_filename, media_uuid, media_filename, media2_uuid, media2_filename) = row;

        let Some(att_uuid) = att_uuid else { continue };

        let filename = media2_filename
            .or(media_filename)
            .or(att_filename)
            .or(att_title)
            .unwrap_or_default();

        // Try to find the file on disk — check deeper media chain first
        let mut file_path = None;
        if let Some(ref m2uuid) = media2_uuid {
            file_path = find_attachment_file(notes_base, m2uuid);
        }
        if file_path.is_none() {
            if let Some(ref muuid) = media_uuid {
                file_path = find_attachment_file(notes_base, muuid);
            }
        }
        if file_path.is_none() {
            file_path = find_attachment_file(notes_base, &att_uuid);
        }

        cache.entries.insert(
            att_uuid,
            AttachmentEntry {
                file_path,
                filename,
                type_uti: type_uti.unwrap_or_default(),
            },
        );
    }

    cache
}

/// Recursively find the first real content file inside a directory (max depth 3).
/// Skips macOS metadata files (.DS_Store, ._ prefix) that would otherwise be
/// returned as the "first" file, preventing the actual image from being found.
fn find_file_recursive(dir: &Path, max_depth: u32) -> Option<PathBuf> {
    if max_depth == 0 || !dir.is_dir() {
        return None;
    }
    let entries = fs::read_dir(dir).ok()?;
    let mut fallback: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            // Skip macOS metadata files
            if name == ".DS_Store" || name.starts_with("._") {
                continue;
            }
            // Prefer image/media files over other files
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let is_media = matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "tiff" | "tif"
                    | "bmp" | "webp" | "svg" | "pdf" | "mov" | "mp4" | "m4v"
                    | "m4a" | "mp3" | "aac" | "wav"
            );
            if is_media {
                return Some(p);
            }
            if fallback.is_none() {
                fallback = Some(p);
            }
        } else if p.is_dir() {
            if let Some(found) = find_file_recursive(&p, max_depth - 1) {
                return Some(found);
            }
        }
    }
    fallback
}

/// Locate an attachment file on disk using a UUID.
fn find_attachment_file(notes_base: &Path, uuid: &str) -> Option<PathBuf> {
    let accounts = notes_base.join("Accounts");
    if accounts.is_dir() {
        if let Ok(entries) = fs::read_dir(&accounts) {
            for entry in entries.flatten() {
                let acct = entry.path();
                // Accounts/*/Media/{uuid}/
                let media_dir = acct.join("Media").join(uuid);
                if let Some(f) = find_file_recursive(&media_dir, 3) {
                    return Some(f);
                }
                // Accounts/*/FallbackImages/{uuid}/
                let fallback_dir = acct.join("FallbackImages").join(uuid);
                if let Some(f) = find_file_recursive(&fallback_dir, 3) {
                    return Some(f);
                }
                // Accounts/*/Previews/{uuid}/
                let previews_dir = acct.join("Previews").join(uuid);
                if let Some(f) = find_file_recursive(&previews_dir, 3) {
                    return Some(f);
                }
            }
        }
    }

    // Direct Media path
    let media = notes_base.join("Media").join(uuid);
    if let Some(f) = find_file_recursive(&media, 3) {
        return Some(f);
    }

    // FallbackImages
    let fallback = notes_base.join("FallbackImages").join(uuid);
    if let Some(f) = find_file_recursive(&fallback, 3) {
        return Some(f);
    }

    None
}

// ============================================================================
// Export logic
// ============================================================================

/// Run the full Apple Notes → Obsidian export (blocking, returns when done).
pub fn run_export(config: &ExportConfig) -> ExportLog {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(ExportLog::new()));
    run_export_into(config, &shared);
    match std::sync::Arc::try_unwrap(shared) {
        Ok(mutex) => mutex.into_inner().unwrap(),
        Err(arc) => {
            let guard = arc.lock().unwrap();
            ExportLog {
                lines: guard.lines.clone(),
                notes_exported: guard.notes_exported,
                attachments_copied: guard.attachments_copied,
                folders_created: guard.folders_created,
                errors: guard.errors,
                is_complete: guard.is_complete,
            }
        }
    }
}

/// Run the full export, writing progress into a shared log.
fn run_export_into(config: &ExportConfig, shared_log: &SharedExportLog) {
    // Helper macro to lock and write to the shared log
    macro_rules! log_msg {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            shared_log.lock().unwrap().log(msg);
        }};
    }
    macro_rules! log_err {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            shared_log.lock().unwrap().error(msg);
        }};
    }

    // Open database
    let conn = match apple::open_db() {
        Ok(c) => c,
        Err(e) => {
            log_err!("{e}");
            shared_log.lock().unwrap().is_complete = true;
            return;
        }
    };

    let notes_base = match apple::notes_base_path() {
        Some(p) => p,
        None => {
            log_err!("Apple Notes base path not found");
            shared_log.lock().unwrap().is_complete = true;
            return;
        }
    };

    // Create output dirs
    if let Err(e) = fs::create_dir_all(&config.output_dir) {
        log_err!("Cannot create output directory: {e}");
        shared_log.lock().unwrap().is_complete = true;
        return;
    }
    let att_dir = config.output_dir.join(&config.attachments_folder);
    if let Err(e) = fs::create_dir_all(&att_dir) {
        log_err!("Cannot create attachments directory: {e}");
        shared_log.lock().unwrap().is_complete = true;
        return;
    }

    // Build folder hierarchy
    log_msg!("Building folder hierarchy...");
    let all_folders = match build_folder_hierarchy(&conn) {
        Ok(f) => f,
        Err(e) => {
            log_err!("Failed to build folder hierarchy: {e}");
            shared_log.lock().unwrap().is_complete = true;
            return;
        }
    };

    // Apply folder filter if set (sub-tree export)
    let folders: Vec<FolderInfo> = if let Some(ref filter) = config.folder_filter {
        let filter_set: std::collections::HashSet<i64> = filter.iter().copied().collect();
        let filtered: Vec<FolderInfo> = all_folders
            .into_iter()
            .filter(|f| filter_set.contains(&f.pk))
            .collect();
        log_msg!("  Exporting {} of {} folders (sub-tree)", filtered.len(), filter.len());
        filtered
    } else {
        let len = all_folders.len();
        log_msg!("  Found {} folders", len);
        all_folders
    };

    // Build attachment cache
    log_msg!("Building attachment cache...");
    let att_cache = build_attachment_cache(&conn, &notes_base);
    let found_files = att_cache
        .entries
        .values()
        .filter(|e| e.file_path.is_some())
        .count();
    let missing_count = att_cache.entries.len() - found_files;
    log_msg!(
        "  {} attachments in DB, {} files on disk, {} not found",
        att_cache.entries.len(),
        found_files,
        missing_count
    );
    if missing_count > 0 {
        log_msg!("");
        log_msg!("Missing attachments (no file on disk):");
        let mut missing: Vec<_> = att_cache
            .entries
            .iter()
            .filter(|(_, e)| e.file_path.is_none())
            .collect();
        missing.sort_by_key(|(uuid, _)| (*uuid).clone());
        for (uuid, entry) in &missing {
            let name = if entry.filename.is_empty() {
                &entry.type_uti
            } else {
                &entry.filename
            };
            log_msg!("  {uuid}  ({name})");
        }
    }
    log_msg!("");

    // Track used filenames for deduplication
    let mut used_filenames: HashMap<String, HashSet<String>> = HashMap::new();
    let mut used_att_names: HashSet<String> = HashSet::new();

    // Export each folder
    for folder in &folders {
        let notes = match apple::get_notes_in_folder(&conn, folder.pk) {
            Ok(n) => n,
            Err(e) => {
                log_err!("Error querying folder '{}': {e}", folder.title);
                continue;
            }
        };

        if notes.is_empty() {
            continue;
        }

        let folder_dir = config.output_dir.join(&folder.rel_path);
        if !folder_dir.exists() {
            if let Err(e) = fs::create_dir_all(&folder_dir) {
                log_err!("Cannot create folder '{}': {e}", folder.rel_path);
                continue;
            }
            shared_log.lock().unwrap().folders_created += 1;
        }

        log_msg!(
            "Folder: {} ({} notes)",
            folder.rel_path,
            notes.len()
        );

        for note in &notes {
            export_note(
                note,
                &folder_dir,
                &att_dir,
                &config.attachments_folder,
                &att_cache,
                &mut used_filenames,
                &mut used_att_names,
                shared_log,
            );
        }
    }

    let mut log = shared_log.lock().unwrap();
    let folders_created = log.folders_created;
    let notes_exported = log.notes_exported;
    let attachments_copied = log.attachments_copied;
    let errors = log.errors;
    log.log(String::new());
    log.log("═══════════════════════════════════════════");
    log.log("Export complete!");
    log.log(format!("  Folders created: {folders_created}"));
    log.log(format!("  Notes exported:  {notes_exported}"));
    log.log(format!("  Attachments:     {attachments_copied}"));
    log.log(format!("  Errors:          {errors}"));
    log.is_complete = true;
}

#[allow(clippy::too_many_arguments)]
fn export_note(
    note: &apple::NoteRow,
    folder_dir: &Path,
    att_dir: &Path,
    att_folder_name: &str,
    att_cache: &AttachmentCache,
    used_filenames: &mut HashMap<String, HashSet<String>>,
    used_att_names: &mut HashSet<String>,
    log: &SharedExportLog,
) {
    let title = note
        .title
        .as_deref()
        .unwrap_or_else(|| {
            note.snippet.as_deref().unwrap_or("Untitled")
        });
    // Truncate to ~50 chars at a char boundary
    let title = if title.len() > 50 {
        match title.char_indices().nth(50) {
            Some((idx, _)) => &title[..idx],
            None => title,
        }
    } else {
        title
    };
    let title = title.lines().next().unwrap_or("Untitled");
    let safe_title = apple::sanitize_filename(title);

    let filename = unique_filename(
        folder_dir,
        &format!("{safe_title}.md"),
        used_filenames,
    );
    let filepath = folder_dir.join(&filename);

    // Extract text, attachments, and formatting from protobuf
    let (text, proto_attachments, style_runs) = match &note.data {
        Some(data) => apple::extract_from_zdata_styled(data),
        None => {
            let t = note.snippet.clone().unwrap_or_default();
            (t, Vec::new(), Vec::new())
        }
    };

    // Build a map from character position → Obsidian link for each attachment.
    // Using position-based matching (not sequential) because skipped attachment
    // types (hashtags, mentions) leave U+FFFC in the text but have no link entry.
    let mut attachment_map: HashMap<usize, String> = HashMap::new();
    for att_info in &proto_attachments {
        if let Some(link) = copy_attachment(
            &att_info.uuid,
            &att_info.type_uti,
            att_dir,
            att_folder_name,
            att_cache,
            used_att_names,
            log,
        ) {
            attachment_map.insert(att_info.position, link);
        }
    }

    // Convert to Markdown with formatting and embedded attachments
    let text = apply_markdown_formatting(&text, &style_runs, &attachment_map);

    // Write file
    match fs::write(&filepath, &text) {
        Ok(()) => {
            let mut l = log.lock().unwrap();
            l.log(format!("  Exported: {filename}"));
            l.notes_exported += 1;
        }
        Err(e) => {
            log.lock().unwrap().error(format!("  Failed to write '{filename}': {e}"));
            return;
        }
    }

    // Set file modification time
    if let Some(modified) = note.modified {
        let unix_ts = apple::cocoa_to_unix(modified);
        if unix_ts > 0.0 {
            let ft = filetime::FileTime::from_unix_time(unix_ts as i64, 0);
            let _ = filetime::set_file_mtime(&filepath, ft);
        }
    }
}


/// Convert plain text + style runs + attachment map into formatted Markdown.
pub(crate) fn apply_markdown_formatting(
    text: &str,
    style_runs: &[apple::StyleRun],
    attachment_map: &HashMap<usize, String>,
) -> String {
    if style_runs.is_empty() {
        // No formatting info — just handle attachments and return
        return apply_attachments_only(text, attachment_map);
    }

    // Build a per-character style map from the runs
    let chars: Vec<char> = text.chars().collect();
    let char_count = chars.len();

    // For each character, store its style run index
    let mut char_style: Vec<usize> = vec![0; char_count];
    for (run_idx, run) in style_runs.iter().enumerate() {
        let start = run.offset.min(char_count);
        let end = (run.offset + run.length).min(char_count);
        for i in start..end {
            char_style[i] = run_idx;
        }
    }

    // Process line by line, applying paragraph and inline formatting.
    // We split the text into lines, then for each line determine what formatting applies.
    let mut output = String::with_capacity(text.len() * 2);
    let mut in_code_block = false;
    let mut numbered_counter: u32 = 0;
    let mut prev_was_list = false;
    let mut in_callout = false;

    // Split into lines by tracking newline characters
    let mut line_start = 0;
    loop {
        // Find the end of this line
        let line_end = chars[line_start..].iter().position(|&c| c == '\n')
            .map(|p| line_start + p)
            .unwrap_or(char_count);

        // Get the paragraph style for this line from the trailing newline's run.
        // Apple Notes stores paragraph style on the run covering the \n that
        // terminates the paragraph, not on the run covering the text characters.
        let line_run_idx = if line_end < char_count {
            // Use the run covering the trailing newline
            char_style[line_end]
        } else if line_start < char_count {
            // Last line (no trailing \n) — use the last character's run
            char_style[char_count - 1]
        } else {
            0
        };
        let run = style_runs.get(line_run_idx);
        let para = run.map(|r| r.paragraph).unwrap_or(apple::ParagraphStyle::Body);
        let indent = run.map(|r| r.indent).unwrap_or(0);

        let is_list = matches!(
            para,
            apple::ParagraphStyle::BulletList
                | apple::ParagraphStyle::DashedList
                | apple::ParagraphStyle::NumberedList
                | apple::ParagraphStyle::Checkbox { .. }
        );
        let is_monospaced = para == apple::ParagraphStyle::Monospaced;

        // Track numbered list counter
        if para == apple::ParagraphStyle::NumberedList {
            if !prev_was_list {
                numbered_counter = 0;
            }
            numbered_counter += 1;
        } else if !is_list {
            numbered_counter = 0;
        }

        // Handle code block transitions
        if is_monospaced && !in_code_block {
            output.push_str("```\n");
            in_code_block = true;
        } else if !is_monospaced && in_code_block {
            output.push_str("```\n");
            in_code_block = false;
        }

        // Build the line content with inline formatting
        let mut line_content = String::new();
        let mut pos = line_start;
        while pos < line_end {
            let ch = chars[pos];
            if ch == apple::ATTACHMENT_MARKER {
                // Look up attachment by character position
                if let Some(link) = attachment_map.get(&pos) {
                    if !line_content.is_empty() {
                        line_content.push('\n');
                    }
                    line_content.push_str(link);
                    // Ensure a newline after the link so subsequent chars
                    // don't get appended to the path/link text
                    line_content.push('\n');
                }
                pos += 1;
                continue;
            }

            if in_code_block {
                // No inline formatting inside code blocks
                line_content.push(ch);
                pos += 1;
                continue;
            }

            // Collect a contiguous run of characters with the same inline style
            let run_i = if pos < char_count { char_style[pos] } else { 0 };
            let cur_run = style_runs.get(run_i);
            let cur_inline = cur_run.map(|r| r.inline_style).unwrap_or_default();
            let cur_link = cur_run.and_then(|r| r.link.as_deref());

            let mut span = String::new();
            while pos < line_end {
                let c = chars[pos];
                if c == apple::ATTACHMENT_MARKER {
                    break;
                }
                let ri = if pos < char_count { char_style[pos] } else { 0 };
                let r = style_runs.get(ri);
                let inline = r.map(|r| r.inline_style).unwrap_or_default();
                let link = r.and_then(|r| r.link.as_deref());
                if inline.bold != cur_inline.bold
                    || inline.italic != cur_inline.italic
                    || inline.strikethrough != cur_inline.strikethrough
                    || link != cur_link
                {
                    break;
                }
                span.push(c);
                pos += 1;
            }

            if span.is_empty() {
                continue;
            }

            // Apply inline formatting wrappers
            let mut formatted = span;
            if cur_inline.strikethrough {
                formatted = format!("~~{formatted}~~");
            }
            if cur_inline.bold && cur_inline.italic {
                formatted = format!("***{formatted}***");
            } else if cur_inline.bold {
                formatted = format!("**{formatted}**");
            } else if cur_inline.italic {
                formatted = format!("*{formatted}*");
            }
            if let Some(url) = cur_link {
                formatted = format!("[{formatted}]({url})");
            }
            line_content.push_str(&formatted);
        }

        // Check for dash patterns and callout blocks
        let trimmed = line_content.trim();
        let is_dash_line = !trimmed.is_empty()
            && trimmed.chars().all(|c| c == '-' || c == '\u{2014}' || c == '\u{2013}');
        let dash_count = trimmed.chars().filter(|c| *c == '-' || *c == '\u{2014}' || *c == '\u{2013}').count();

        // Detect "--- notes ----" opener: contains "notes" and the rest is dashes/spaces
        let is_notes_opener = !in_code_block && !is_dash_line && {
            let lower = trimmed.to_lowercase();
            if lower.contains("notes") {
                let without_notes = lower.replace("notes", "");
                let remaining = without_notes.trim();
                remaining.is_empty()
                    || remaining.chars().all(|c| c == '-' || c == '\u{2014}' || c == '\u{2013}' || c == ' ')
            } else {
                false
            }
        };

        if is_notes_opener && !in_callout {
            // Start an info callout block
            in_callout = true;
            output.push_str("> [!info]");
        } else if in_callout && is_dash_line && dash_count >= 4 {
            // 4+ dashes on a line by themselves closes the callout
            in_callout = false;
        } else if is_dash_line && !in_code_block {
            output.push_str("---");
        } else if in_code_block {
            // Inside code block, no prefix
            output.push_str(&line_content);
        } else if trimmed.is_empty() && !is_list {
            // Don't add heading prefixes to blank lines, but keep empty list items for spacing
            if in_callout {
                output.push('>');
            }
        } else {
            // Callout prefix
            let callout_prefix = if in_callout { "> " } else { "" };
            // Apply paragraph-level prefix
            let indent_str = "  ".repeat(indent as usize);
            match para {
                apple::ParagraphStyle::Title => {
                    output.push_str(callout_prefix);
                    output.push_str("# ");
                    output.push_str(&line_content);
                }
                apple::ParagraphStyle::Heading => {
                    output.push_str(callout_prefix);
                    output.push_str("## ");
                    output.push_str(&line_content);
                }
                apple::ParagraphStyle::Subheading => {
                    output.push_str(callout_prefix);
                    output.push_str("### ");
                    output.push_str(&line_content);
                }
                apple::ParagraphStyle::BulletList | apple::ParagraphStyle::DashedList => {
                    output.push_str(callout_prefix);
                    output.push_str(&indent_str);
                    output.push_str("- ");
                    output.push_str(&line_content);
                }
                apple::ParagraphStyle::NumberedList => {
                    output.push_str(callout_prefix);
                    output.push_str(&indent_str);
                    output.push_str(&format!("{numbered_counter}. "));
                    output.push_str(&line_content);
                }
                apple::ParagraphStyle::Checkbox { checked } => {
                    output.push_str(callout_prefix);
                    output.push_str(&indent_str);
                    if checked {
                        output.push_str("- [x] ");
                    } else {
                        output.push_str("- [ ] ");
                    }
                    output.push_str(&line_content);
                }
                apple::ParagraphStyle::Monospaced | apple::ParagraphStyle::Body => {
                    output.push_str(callout_prefix);
                    output.push_str(&line_content);
                }
            }
        }

        prev_was_list = is_list;
        output.push('\n');

        // Move past the newline
        if line_end < char_count {
            line_start = line_end + 1;
        } else {
            break;
        }
    }

    // Close trailing code block
    if in_code_block {
        output.push_str("```\n");
    }

    // No remaining attachments to handle — position-based lookup covers all

    // Clean up
    let mut text = output.replace("\r\n", "\n").replace('\r', "\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim().to_string()
}

/// Fallback: just handle attachment substitution without formatting.
fn apply_attachments_only(text: &str, attachment_map: &HashMap<usize, String>) -> String {
    let mut result = String::with_capacity(text.len());
    for (pos, ch) in text.chars().enumerate() {
        if ch == apple::ATTACHMENT_MARKER {
            if let Some(link) = attachment_map.get(&pos) {
                result.push_str("\n\n");
                result.push_str(link);
                result.push_str("\n\n");
            }
        } else {
            result.push(ch);
        }
    }
    let mut text = result.replace("\r\n", "\n").replace('\r', "\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim().to_string()
}

fn copy_attachment(
    uuid: &str,
    type_uti: &str,
    att_dir: &Path,
    att_folder_name: &str,
    cache: &AttachmentCache,
    used_names: &mut HashSet<String>,
    log: &SharedExportLog,
) -> Option<String> {
    let entry = match cache.entries.get(uuid) {
        Some(e) => e,
        None => return None,
    };
    let src_path = match entry.file_path.as_ref() {
        Some(p) => p,
        None => return None,
    };

    if !src_path.exists() {
        return None;
    }

    // Use the actual filename from disk, falling back to database filename.
    // This avoids mismatches where the database stores a UUID-based name
    // but the file on disk has a different name.
    let src_filename = src_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let preferred_name = if !entry.filename.is_empty() {
        // Use database filename but ensure it has the correct extension from the source
        let db_name = &entry.filename;
        let src_ext = src_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let db_ext = Path::new(db_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        // If the database name has no extension or a different one, use source extension
        if db_ext.is_empty() && !src_ext.is_empty() {
            format!("{db_name}.{src_ext}")
        } else {
            db_name.clone()
        }
    } else {
        src_filename
    };

    // Sanitize: strip path separators and invalid characters from the filename
    let sanitized_name = sanitize_attachment_name(&preferred_name);

    let uti = if type_uti.is_empty() {
        &entry.type_uti
    } else {
        type_uti
    };

    // Check if this is a HEIC file that needs conversion
    let src_ext = src_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_heic = src_ext == "heic" || src_ext == "heif";

    // For HEIC files, change the destination name to .jpg
    let final_name = if is_heic {
        let stem = Path::new(&sanitized_name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        format!("{stem}.jpg")
    } else {
        sanitized_name
    };

    let dest_name = unique_attachment_name(&final_name, uti, att_dir, used_names);
    let dest_path = att_dir.join(&dest_name);

    // Try HEIC → JPEG conversion using macOS sips
    let copy_ok = if is_heic {
        match convert_heic_to_jpeg(src_path, &dest_path) {
            Ok(()) => true,
            Err(e) => {
                // Conversion failed — fall back to raw copy with original extension
                log.lock().unwrap().log(format!(
                    "  HEIC conversion failed ({e}), copying original"
                ));
                fs::copy(src_path, &dest_path).is_ok()
            }
        }
    } else {
        match fs::copy(src_path, &dest_path) {
            Ok(bytes) => bytes > 0,
            Err(_) => false,
        }
    };

    if !copy_ok || !dest_path.exists() || dest_path.metadata().map_or(true, |m| m.len() == 0) {
        log.lock().unwrap().error(format!(
            "  Attachment copy produced empty/missing file: {dest_name} (from {})",
            src_path.display()
        ));
        let _ = fs::remove_file(&dest_path);
        return None;
    }

    log.lock().unwrap().attachments_copied += 1;
    let embed = is_embeddable_attachment(uti, &dest_name);
    let link = if embed {
        format!("![[{att_folder_name}/{dest_name}]]")
    } else {
        format!("[[{att_folder_name}/{dest_name}]]")
    };
    Some(link)
}

/// Convert a HEIC/HEIF image to JPEG using macOS `sips`.
fn convert_heic_to_jpeg(src: &Path, dest: &Path) -> Result<(), String> {
    let output = std::process::Command::new("sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "85"])
        .arg(src)
        .arg("--out")
        .arg(dest)
        .output()
        .map_err(|e| format!("sips not available: {e}"))?;

    if output.status.success() && dest.exists() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("sips failed: {stderr}"))
    }
}

/// Sanitize an attachment filename: remove path separators and control characters.
fn sanitize_attachment_name(name: &str) -> String {
    // Take only the filename part (strip any directory components)
    let name = Path::new(name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    let sanitized: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') && !c.is_control())
        .collect();

    if sanitized.is_empty() {
        "attachment".to_string()
    } else {
        sanitized
    }
}

/// Check if an attachment should be embedded (![[…]]) rather than linked ([[…]]).
/// Obsidian embeds images, video, audio, and PDFs.
fn is_embeddable_attachment(type_uti: &str, filename: &str) -> bool {
    let uti_lower = type_uti.to_lowercase();
    if ["image", "jpeg", "png", "gif", "heic", "tiff", "bmp", "webp",
        "video", "movie", "quicktime", "mpeg", "mp4", "avi",
        "audio", "mp3", "aac", "wav", "m4a", "ogg",
        "pdf"]
        .iter()
        .any(|k| uti_lower.contains(k))
    {
        return true;
    }

    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    [
        "jpg", "jpeg", "png", "gif", "webp", "heic", "tiff", "tif", "bmp", "svg",
        "mov", "mp4", "m4v", "avi", "mkv", "webm",
        "mp3", "m4a", "aac", "wav", "ogg", "flac",
        "pdf",
    ]
    .contains(&ext.as_str())
}

fn unique_filename(
    dir: &Path,
    base: &str,
    used: &mut HashMap<String, HashSet<String>>,
) -> String {
    let dir_key = dir.to_string_lossy().to_string();
    let set = used.entry(dir_key).or_default();

    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (base.to_string(), String::new()),
    };

    let mut name = format!("{stem}{ext}");
    let mut counter = 1;
    while set.contains(&name.to_lowercase()) || dir.join(&name).exists() {
        name = format!("{stem}_{counter}{ext}");
        counter += 1;
    }
    set.insert(name.to_lowercase());
    name
}

fn unique_attachment_name(
    base: &str,
    type_uti: &str,
    att_dir: &Path,
    used: &mut HashSet<String>,
) -> String {
    let base = if base.is_empty() {
        let ext = uti_to_extension(type_uti);
        format!("attachment{ext}")
    } else {
        base.to_string()
    };

    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (base.clone(), String::new()),
    };

    let mut name = format!("{stem}{ext}");
    let mut counter = 1;
    while used.contains(&name.to_lowercase()) || att_dir.join(&name).exists() {
        name = format!("{stem}_{counter}{ext}");
        counter += 1;
    }
    used.insert(name.to_lowercase());
    name
}

fn uti_to_extension(uti: &str) -> &str {
    let uti_lower = uti.to_lowercase();
    if uti_lower.contains("jpeg") || uti_lower.contains("jpg") {
        ".jpg"
    } else if uti_lower.contains("png") {
        ".png"
    } else if uti_lower.contains("gif") {
        ".gif"
    } else if uti_lower.contains("pdf") {
        ".pdf"
    } else if uti_lower.contains("heic") {
        ".heic"
    } else {
        ".bin"
    }
}
