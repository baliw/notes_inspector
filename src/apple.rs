use crate::tree::TreeNode;
use flate2::read::GzDecoder;
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

thread_local! {
    /// Cache of rendered note content, keyed by note PK.
    /// Avoids repeated DB queries, protobuf parsing, and attachment resolution.
    static NOTE_CACHE: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
}

/// Clear the note content cache (e.g. after code changes to rendering logic).
#[allow(dead_code)]
pub fn clear_note_cache() {
    NOTE_CACHE.with(|c| c.borrow_mut().clear());
}

// ============================================================================
// Database access
// ============================================================================

/// Default path to the Apple Notes SQLite database on macOS.
fn default_db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home
        .join("Library")
        .join("Group Containers")
        .join("group.com.apple.notes")
        .join("NoteStore.sqlite");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Base path for the Apple Notes container (attachment files live here).
pub fn notes_base_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home
        .join("Library")
        .join("Group Containers")
        .join("group.com.apple.notes");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Check if the Apple Notes database is accessible.
#[allow(dead_code)]
pub fn is_available() -> bool {
    default_db_path().is_some()
}

/// Open a read-only connection to the Apple Notes database.
///
/// Copies the database (and WAL/SHM files) to a temp directory first,
/// because the live WAL-mode database requires write access to its directory.
/// Open a fresh copy of the Apple Notes database (copies DB + WAL + SHM).
/// Use this for initial tree loading.
pub fn open_db() -> Result<Connection, String> {
    let src = default_db_path().ok_or(
        "Apple Notes database not found. Expected at ~/Library/Group Containers/group.com.apple.notes/NoteStore.sqlite"
            .to_string(),
    )?;
    open_db_at(&src)
}

/// Open the already-copied temp database without re-copying.
/// Much faster — use this for subsequent reads after the tree is built.
fn open_db_cached() -> Result<Connection, String> {
    let tmp_db = std::env::temp_dir()
        .join("notes_inspector")
        .join("NoteStore.sqlite");
    if !tmp_db.exists() {
        // Temp copy doesn't exist yet — fall back to full copy
        return open_db();
    }
    let conn = Connection::open(&tmp_db)
        .map_err(|e| format!("Failed to open cached database: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    Ok(conn)
}

fn open_db_at(path: &Path) -> Result<Connection, String> {
    // Copy the database, WAL, and SHM files to a temp directory so SQLite can
    // read the WAL journal (which contains recent, uncheckpointed changes).
    // Without the WAL, deleted items still show and recent edits are missing.
    //
    // We never modify the originals — only the temp copy is opened.
    let tmp_dir = std::env::temp_dir().join("notes_inspector");
    fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("Failed to create temp directory: {e}"))?;

    let db_filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let tmp_db = tmp_dir.join(&db_filename);

    // Copy the main database file
    fs::copy(path, &tmp_db).map_err(|e| {
        if e.raw_os_error() == Some(1) {
            "Permission denied reading Apple Notes database. \
             Grant Full Disk Access to your terminal app: \
             System Settings → Privacy & Security → Full Disk Access"
                .to_string()
        } else {
            format!("Failed to copy database: {e}")
        }
    })?;

    // Copy WAL and SHM files so SQLite sees the full current state
    let parent = path.parent().unwrap_or(path);
    for suffix in ["-wal", "-shm"] {
        let src = parent.join(format!("{db_filename}{suffix}"));
        if src.exists() {
            let dst = tmp_dir.join(format!("{db_filename}{suffix}"));
            let _ = fs::copy(&src, &dst);
        }
    }

    // Open the temp copy — not read-only, so SQLite can replay the WAL
    let conn = Connection::open(&tmp_db)
        .map_err(|e| format!("Failed to open Apple Notes database: {e}"))?;

    // Checkpoint the WAL into the main db file so all data is visible,
    // then switch to a non-WAL journal mode so no further writes occur.
    let _ = conn.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE); \
         PRAGMA journal_mode=DELETE; \
         PRAGMA query_only=ON;",
    );

    Ok(conn)
}

/// Check if a column exists in ZICCLOUDSYNCINGOBJECT.
fn has_column(conn: &Connection, col: &str) -> bool {
    conn.prepare(&format!(
        "SELECT {col} FROM ZICCLOUDSYNCINGOBJECT LIMIT 0"
    ))
    .is_ok()
}

/// SQL fragment to filter out soft-deleted rows (if column exists).
fn delete_filter(conn: &Connection) -> &'static str {
    // Cache-unfriendly but simple; called only a few times during init.
    // The column check is fast.
    if has_column(conn, "ZMARKEDFORDELETION") {
        " AND (ZMARKEDFORDELETION IS NULL OR ZMARKEDFORDELETION != 1)"
    } else {
        ""
    }
}

// ============================================================================
// Tree building (for TUI display)
// ============================================================================

/// Build a tree from Apple Notes by reading the SQLite database directly.
pub fn build_notes_tree() -> Result<TreeNode, String> {
    let conn = open_db()?;
    build_tree_from_db(&conn)
}

/// Build tree from a database at a specific path.
#[allow(dead_code)]
pub fn build_notes_tree_from_path(db_path: &Path) -> Result<TreeNode, String> {
    let conn = open_db_at(db_path)?;
    build_tree_from_db(&conn)
}

fn build_tree_from_db(conn: &Connection) -> Result<TreeNode, String> {
    let mut root = TreeNode::new_folder("Apple Notes".to_string(), PathBuf::new());
    root.expanded = true;

    let df = delete_filter(conn);

    // --- Folders with parent hierarchy ---
    // The ZICCLOUDSYNCINGOBJECT table is polymorphic — ZTITLE2 IS NOT NULL
    // matches real user folders but also account containers ("iCloud",
    // "On My Mac"), system folders ("Recently Deleted"), and other internals.
    //
    // We filter with ZFOLDERTYPE if available:
    //   NULL or 0 = regular user folder
    //   1 = system folder (Recently Deleted, etc.)
    //
    // Folders that end up with zero notes (recursively) are pruned so
    // account-level containers and other empty ghosts don't clutter the tree.
    let has_parent = has_column(conn, "ZPARENT");
    let has_foldertype = has_column(conn, "ZFOLDERTYPE");

    let foldertype_filter = if has_foldertype {
        " AND (ZFOLDERTYPE IS NULL OR ZFOLDERTYPE = 0)"
    } else {
        ""
    };

    let folder_sql = if has_parent {
        format!(
            "SELECT Z_PK, ZTITLE2, ZPARENT FROM ZICCLOUDSYNCINGOBJECT \
             WHERE ZTITLE2 IS NOT NULL{df}{foldertype_filter} ORDER BY ZTITLE2"
        )
    } else {
        format!(
            "SELECT Z_PK, ZTITLE2 FROM ZICCLOUDSYNCINGOBJECT \
             WHERE ZTITLE2 IS NOT NULL{df}{foldertype_filter} ORDER BY ZTITLE2"
        )
    };

    let mut folder_map: HashMap<i64, TreeNode> = HashMap::new();
    let mut parent_map: HashMap<i64, Option<i64>> = HashMap::new();
    let mut folder_order: Vec<i64> = Vec::new();

    let mut stmt = conn
        .prepare(&folder_sql)
        .map_err(|e| format!("Failed to query folders: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            let pk: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let parent: Option<i64> = if has_parent {
                row.get(2)?
            } else {
                None
            };
            Ok((pk, title, parent))
        })
        .map_err(|e| format!("Failed to read folders: {e}"))?;

    for row in rows {
        let (pk, title, parent_pk) =
            row.map_err(|e| format!("Error reading folder row: {e}"))?;
        let node = TreeNode::new_folder(
            title,
            PathBuf::from(format!("apple-notes://folder/{pk}")),
        );
        folder_map.insert(pk, node);
        parent_map.insert(pk, parent_pk);
        folder_order.push(pk);
    }

    // --- Notes ---
    let has_moddate = has_column(conn, "ZMODIFICATIONDATE1");
    let has_pinned = has_column(conn, "ZISPINNED");

    let mut select_cols = vec!["Z_PK", "ZTITLE1", "ZFOLDER"];
    if has_moddate {
        select_cols.push("ZMODIFICATIONDATE1");
    }
    if has_pinned {
        select_cols.push("ZISPINNED");
    }

    let note_sql = format!(
        "SELECT {} FROM ZICCLOUDSYNCINGOBJECT WHERE ZTITLE1 IS NOT NULL{df}",
        select_cols.join(", ")
    );
    let mut note_stmt = conn
        .prepare(&note_sql)
        .map_err(|e| format!("Failed to query notes: {e}"))?;

    let notes = note_stmt
        .query_map([], |row| {
            let pk: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let folder_pk: Option<i64> = row.get(2)?;
            let mut col_idx = 3;
            let mod_date: Option<f64> = if has_moddate {
                let v = row.get(col_idx)?;
                col_idx += 1;
                v
            } else {
                let _ = col_idx;
                None
            };
            let is_pinned: bool = if has_pinned {
                let v: Option<i64> = row.get(col_idx)?;
                v.unwrap_or(0) == 1
            } else {
                false
            };
            Ok((pk, title, folder_pk, mod_date, is_pinned))
        })
        .map_err(|e| format!("Failed to read notes: {e}"))?;

    for note in notes {
        let (pk, title, folder_pk, mod_date, is_pinned) =
            note.map_err(|e| format!("Error reading note row: {e}"))?;
        let mut note_node = TreeNode::new_note(
            title,
            PathBuf::from(format!("apple-notes://note/{pk}")),
        );
        note_node.modified_date = mod_date;
        note_node.is_pinned = is_pinned;

        if let Some(fpk) = folder_pk {
            if let Some(folder) = folder_map.get_mut(&fpk) {
                folder.children.push(note_node);
                continue;
            }
        }
        // Note in a filtered/system folder — skip
    }

    // Build folder nesting via ZPARENT, then attach top-level folders to root.
    // Collect children first to avoid borrow issues.
    let mut children_to_add: HashMap<i64, Vec<TreeNode>> = HashMap::new();
    let mut top_level_pks: Vec<i64> = Vec::new();

    for &pk in &folder_order {
        let parent_pk = parent_map.get(&pk).copied().flatten();
        if let Some(ppk) = parent_pk {
            if folder_map.contains_key(&ppk) {
                children_to_add
                    .entry(ppk)
                    .or_default()
                    .push(folder_map.remove(&pk).unwrap());
                continue;
            }
        }
        top_level_pks.push(pk);
    }

    // Insert children into their parents (multiple passes for deep nesting)
    for _ in 0..10 {
        let pks: Vec<i64> = children_to_add.keys().copied().collect();
        let mut did_work = false;
        for ppk in pks {
            if let Some(parent) = folder_map.get_mut(&ppk) {
                if let Some(children) = children_to_add.remove(&ppk) {
                    parent.children.extend(children);
                    did_work = true;
                }
            }
        }
        if !did_work {
            break;
        }
    }

    // Only add folders that contain at least one note (recursively).
    // This prunes account containers and other empty ghosts.
    for pk in &top_level_pks {
        if let Some(folder) = folder_map.remove(pk) {
            if folder.count_notes() > 0 {
                root.children.push(folder);
            }
        }
    }

    root.sort_children();
    Ok(root)
}

// ============================================================================
// Note content reading
// ============================================================================

/// Read the content of an Apple Note by its path (encodes Z_PK).
/// Path format: "apple-notes://note/{Z_PK}".
/// Results are cached per PK to avoid repeated DB/protobuf/attachment work.
pub fn read_note(note_path: &str) -> String {
    let pk: i64 = match note_path.strip_prefix("apple-notes://note/") {
        Some(pk_str) => match pk_str.parse() {
            Ok(pk) => pk,
            Err(_) => return format!("Invalid note ID: {note_path}"),
        },
        None => return format!("Invalid note path: {note_path}"),
    };

    // Check cache first
    let cached = NOTE_CACHE.with(|c| c.borrow().get(&pk).cloned());
    if let Some(content) = cached {
        return content;
    }

    let conn = match open_db_cached() {
        Ok(c) => c,
        Err(e) => return e,
    };

    let content = read_note_by_pk(&conn, pk);

    // Store in cache
    NOTE_CACHE.with(|c| {
        c.borrow_mut().insert(pk, content.clone());
    });

    content
}

fn read_note_by_pk(conn: &Connection, note_pk: i64) -> String {
    let has_mergeable = conn
        .prepare("SELECT ZMERGEABLEDATA FROM ZICNOTEDATA LIMIT 0")
        .is_ok();
    let sql = if has_mergeable {
        "SELECT COALESCE(ZDATA, ZMERGEABLEDATA) FROM ZICNOTEDATA WHERE ZNOTE = ?1"
    } else {
        "SELECT ZDATA FROM ZICNOTEDATA WHERE ZNOTE = ?1"
    };
    let result = conn.query_row(sql, [note_pk], |row| {
        let data: Option<Vec<u8>> = row.get(0)?;
        Ok(data)
    });

    let data = match result {
        Ok(Some(data)) => data,
        Ok(None) => return "Note has no content.".to_string(),
        Err(e) => return format!("Error reading note content: {e}"),
    };

    let decompressed = match decompress_gzip(&data) {
        Some(d) => d,
        None => return "Could not decompress note content.".to_string(),
    };

    let (text, attachments, style_runs) = extract_note_full(&decompressed);
    if text.is_empty() {
        return "Could not decode note content.".to_string();
    }

    // Build attachment position map for inline image markers
    let att_map = if attachments.is_empty() {
        HashMap::new()
    } else {
        let att_paths = resolve_attachment_paths(conn, &attachments);
        let mut map = HashMap::new();
        for (att, path) in attachments.iter().zip(att_paths.iter()) {
            let replacement = match path {
                Some(p) => format!("__INLINE_IMAGE__:{p}"),
                None => format!("[Attachment not found: {}]", att.uuid),
            };
            map.insert(att.position, replacement);
        }
        map
    };

    // Apply paragraph-level and inline formatting from style runs
    let formatted = crate::export::apply_markdown_formatting(&text, &style_runs, &att_map);

    // Add hard breaks (trailing "  ") only to plain body text lines.
    // List items, headings, code fences, and blank lines must be left alone
    // so pulldown-cmark parses them as proper block-level elements.
    let mut result = String::with_capacity(formatted.len() + 256);
    for line in formatted.split('\n') {
        let trimmed = line.trim_start();
        let is_block = trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("- [")
            || trimmed.starts_with("```")
            || trimmed.starts_with("---")
            || trimmed.starts_with('>')
            || trimmed.is_empty()
            || trimmed.starts_with("__INLINE_IMAGE__:")
            || trimmed.chars().next().map_or(false, |c| {
                c.is_ascii_digit() && trimmed.contains(". ")
            });
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
        if !is_block {
            result.push_str("  ");
        }
    }
    result
}

/// Resolve attachment UUIDs to file paths on disk.
fn resolve_attachment_paths(
    conn: &Connection,
    attachments: &[AttachmentPosition],
) -> Vec<Option<String>> {
    let notes_base = match notes_base_path() {
        Some(p) => p,
        None => return vec![None; attachments.len()],
    };

    // Primary lookup: attachment → media object
    let sql_media = "\
        SELECT m.ZIDENTIFIER \
        FROM ZICCLOUDSYNCINGOBJECT a \
        LEFT JOIN ZICCLOUDSYNCINGOBJECT m ON a.ZMEDIA = m.Z_PK \
        WHERE a.ZIDENTIFIER = ?1";

    // Fallback: look up all UUIDs associated with the attachment row
    // (ZIDENTIFIER of the attachment itself, plus any linked media chain)
    let sql_chain = "\
        SELECT a.ZIDENTIFIER, m.ZIDENTIFIER, m2.ZIDENTIFIER \
        FROM ZICCLOUDSYNCINGOBJECT a \
        LEFT JOIN ZICCLOUDSYNCINGOBJECT m ON a.ZMEDIA = m.Z_PK \
        LEFT JOIN ZICCLOUDSYNCINGOBJECT m2 ON m.ZMEDIA = m2.Z_PK \
        WHERE a.ZIDENTIFIER = ?1";

    attachments
        .iter()
        .map(|att| {
            // 1. Try media UUID (most common path)
            let media_uuid: Option<String> = conn
                .query_row(sql_media, [&att.uuid], |row| row.get(0))
                .ok()
                .flatten();

            if let Some(ref muuid) = media_uuid {
                if let Some(path) = find_attachment_on_disk(&notes_base, muuid) {
                    return Some(path.to_string_lossy().to_string());
                }
            }

            // 2. Try the attachment UUID directly
            if let Some(path) = find_attachment_on_disk(&notes_base, &att.uuid) {
                return Some(path.to_string_lossy().to_string());
            }

            // 3. Try deeper media chain (media → media)
            if let Ok(m2_uuid) = conn.query_row(sql_chain, [&att.uuid], |row| {
                row.get::<_, Option<String>>(2)
            }) {
                if let Some(ref m2) = m2_uuid {
                    if let Some(path) = find_attachment_on_disk(&notes_base, m2) {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }

            // 4. Try looking up by Z_PK if the UUID looks numeric
            // (some protobuf entries reference PKs, not UUIDs)
            if let Ok(pk) = att.uuid.parse::<i64>() {
                let pk_uuid: Option<String> = conn
                    .query_row(
                        "SELECT ZIDENTIFIER FROM ZICCLOUDSYNCINGOBJECT WHERE Z_PK = ?1",
                        [pk],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten();
                if let Some(ref puuid) = pk_uuid {
                    if let Some(path) = find_attachment_on_disk(&notes_base, puuid) {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }

            None
        })
        .collect()
}

/// Locate an attachment file on disk by UUID.
fn find_attachment_on_disk(notes_base: &Path, uuid: &str) -> Option<PathBuf> {
    let accounts = notes_base.join("Accounts");
    if accounts.is_dir() {
        if let Ok(entries) = fs::read_dir(&accounts) {
            for entry in entries.flatten() {
                let acct = entry.path();
                // Search in Accounts/*/Media/{uuid}/
                let media_dir = acct.join("Media").join(uuid);
                if let Some(f) = find_file_in_dir(&media_dir, 3) {
                    return Some(f);
                }
                // Search in Accounts/*/FallbackImages/{uuid}/
                let fallback_dir = acct.join("FallbackImages").join(uuid);
                if let Some(f) = find_file_in_dir(&fallback_dir, 3) {
                    return Some(f);
                }
                // Search in Accounts/*/Previews/{uuid}/
                let previews_dir = acct.join("Previews").join(uuid);
                if let Some(f) = find_file_in_dir(&previews_dir, 3) {
                    return Some(f);
                }
            }
        }
    }
    let media = notes_base.join("Media").join(uuid);
    if let Some(f) = find_file_in_dir(&media, 3) {
        return Some(f);
    }
    None
}

/// Recursively find the first real content file inside a directory.
/// Skips macOS metadata files (.DS_Store, ._ prefix) and prefers media files.
fn find_file_in_dir(dir: &Path, max_depth: u32) -> Option<PathBuf> {
    if max_depth == 0 || !dir.is_dir() {
        return None;
    }
    let entries = fs::read_dir(dir).ok()?;
    let mut fallback: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            if name == ".DS_Store" || name.starts_with("._") {
                continue;
            }
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
            if let Some(found) = find_file_in_dir(&p, max_depth - 1) {
                return Some(found);
            }
        }
    }
    fallback
}

// ============================================================================
// Protobuf parser (minimal, handles Apple Notes wire format)
// ============================================================================

/// A parsed protobuf field.
#[derive(Debug, Clone)]
pub struct ProtoField {
    pub field_number: u32,
    pub wire_type: u8,
    /// Raw bytes of the value. For varints, this is the encoded bytes.
    /// For length-delimited, this is the payload (without length prefix).
    pub data: Vec<u8>,
}

/// Read a varint, returning (value, bytes_consumed).
fn read_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in data.iter().enumerate() {
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

/// Parse all protobuf fields from a message.
pub fn parse_proto_fields(data: &[u8]) -> Vec<ProtoField> {
    let mut fields = Vec::new();
    let mut offset = 0;
    let mut iterations = 0;

    while offset < data.len() && iterations < 100_000 {
        iterations += 1;

        let Some((tag, tag_len)) = read_varint(&data[offset..]) else {
            break;
        };
        offset += tag_len;

        let field_number = (tag >> 3) as u32;
        let wire_type = (tag & 0x07) as u8;

        if field_number == 0 {
            break;
        }

        match wire_type {
            0 => {
                // Varint
                let Some((val, val_len)) = read_varint(&data[offset..]) else {
                    break;
                };
                fields.push(ProtoField {
                    field_number,
                    wire_type,
                    data: val.to_le_bytes().to_vec(),
                });
                offset += val_len;
            }
            1 => {
                // Fixed64
                if offset + 8 > data.len() {
                    break;
                }
                fields.push(ProtoField {
                    field_number,
                    wire_type,
                    data: data[offset..offset + 8].to_vec(),
                });
                offset += 8;
            }
            2 => {
                // Length-delimited
                let Some((length, len_len)) = read_varint(&data[offset..]) else {
                    break;
                };
                offset += len_len;
                let length = length as usize;
                if offset + length > data.len() {
                    break;
                }
                fields.push(ProtoField {
                    field_number,
                    wire_type,
                    data: data[offset..offset + length].to_vec(),
                });
                offset += length;
            }
            5 => {
                // Fixed32
                if offset + 4 > data.len() {
                    break;
                }
                fields.push(ProtoField {
                    field_number,
                    wire_type,
                    data: data[offset..offset + 4].to_vec(),
                });
                offset += 4;
            }
            _ => break,
        }
    }

    fields
}

/// Get first length-delimited field with the given number (as bytes).
pub fn proto_get_bytes(fields: &[ProtoField], num: u32) -> Option<&[u8]> {
    fields
        .iter()
        .find(|f| f.field_number == num && f.wire_type == 2)
        .map(|f| f.data.as_slice())
}

/// Get all length-delimited fields with the given number.
pub fn proto_get_all_bytes(fields: &[ProtoField], num: u32) -> Vec<&[u8]> {
    fields
        .iter()
        .filter(|f| f.field_number == num && f.wire_type == 2)
        .map(|f| f.data.as_slice())
        .collect()
}

/// Get first varint field with the given number.
pub fn proto_get_varint(fields: &[ProtoField], num: u32) -> Option<u64> {
    fields
        .iter()
        .find(|f| f.field_number == num && f.wire_type == 0)
        .map(|f| u64::from_le_bytes(f.data[..8].try_into().unwrap_or([0; 8])))
}

/// Get first string (length-delimited, interpreted as UTF-8) field.
pub fn proto_get_string(fields: &[ProtoField], num: u32) -> Option<String> {
    proto_get_bytes(fields, num)
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
}

// ============================================================================
// Note content extraction with attachment positions
// ============================================================================

/// Attachment position within note text.
#[derive(Debug, Clone)]
pub struct AttachmentPosition {
    /// Character offset in the note text.
    pub position: usize,
    /// Attachment UUID (matches ZIDENTIFIER in ZICCLOUDSYNCINGOBJECT).
    pub uuid: String,
    /// Type UTI (e.g., "public.jpeg").
    pub type_uti: String,
}

/// Paragraph-level style from Apple Notes protobuf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParagraphStyle {
    Body,
    Title,
    Heading,
    Subheading,
    Monospaced,
    BulletList,
    DashedList,
    NumberedList,
    Checkbox { checked: bool },
}

/// Inline formatting flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// A styled run of text from the protobuf AttributeRun.
#[derive(Debug, Clone)]
pub struct StyleRun {
    /// Character offset in the note text.
    pub offset: usize,
    /// Number of characters in this run.
    pub length: usize,
    /// Paragraph-level style (headers, lists, etc.).
    pub paragraph: ParagraphStyle,
    /// Indent level (0-based).
    pub indent: u32,
    /// Inline formatting (bold, italic, etc.).
    pub inline_style: InlineStyle,
    /// Optional hyperlink URL.
    pub link: Option<String>,
}

/// Attachment UTIs that are inline text formatting, not file attachments.
const SKIP_ATTACHMENT_TYPES: &[&str] = &[
    "com.apple.notes.inlinetextattachment.hashtag",
    "com.apple.notes.inlinetextattachment.mention",
];

/// Unicode Object Replacement Character — marks attachment positions in note text.
pub const ATTACHMENT_MARKER: char = '\u{FFFC}';

/// Decompress gzip data.
pub fn decompress_gzip(data: &[u8]) -> Option<Vec<u8>> {
    // Check gzip magic
    if data.len() < 2 || data[0] != 0x1f || data[1] != 0x8b {
        // Not gzipped, return as-is
        return Some(data.to_vec());
    }
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Extract note text and attachment positions from decompressed protobuf data.
///
/// Protobuf path: root → field 2 (Note) → field 3 (Content) → field 2 (text)
/// Attachments:   root → field 2 (Note) → field 3 (Content) → field 5 (AttributeRun, repeated)
pub fn extract_note_content_with_attachments(data: &[u8]) -> (String, Vec<AttachmentPosition>) {
    let (text, attachments, _styles) = extract_note_full(data);
    (text, attachments)
}

/// Extract note text, attachment positions, and style runs from decompressed protobuf data.
pub fn extract_note_full(data: &[u8]) -> (String, Vec<AttachmentPosition>, Vec<StyleRun>) {
    let doc_fields = parse_proto_fields(data);

    let Some(note_data) = proto_get_bytes(&doc_fields, 2) else {
        return (String::new(), Vec::new(), Vec::new());
    };
    let note_fields = parse_proto_fields(note_data);

    let Some(content_data) = proto_get_bytes(&note_fields, 3) else {
        return (String::new(), Vec::new(), Vec::new());
    };
    let content_fields = parse_proto_fields(content_data);

    // Field 2: the note text
    let text = proto_get_string(&content_fields, 2).unwrap_or_default();

    // Field 5: repeated AttributeRun
    let attr_runs = proto_get_all_bytes(&content_fields, 5);
    let mut style_runs = Vec::new();
    let mut current_pos: usize = 0;

    // First pass: collect ALL attachment info (including skipped types) in protobuf order,
    // and collect style runs. We'll match to actual FFFC positions afterward.
    struct RawAttachment {
        uuid: String,
        type_uti: String,
        skipped: bool,
    }
    let mut raw_attachments: Vec<RawAttachment> = Vec::new();

    for run_data in attr_runs {
        let run_fields = parse_proto_fields(run_data);

        let length = proto_get_varint(&run_fields, 1).unwrap_or(1) as usize;

        // Field 12: AttachmentInfo (optional)
        if let Some(att_data) = proto_get_bytes(&run_fields, 12) {
            let att_fields = parse_proto_fields(att_data);
            let uuid = proto_get_string(&att_fields, 1).unwrap_or_default();
            let type_uti = proto_get_string(&att_fields, 2).unwrap_or_default();

            if !uuid.is_empty() {
                let skipped = SKIP_ATTACHMENT_TYPES.contains(&type_uti.as_str());
                raw_attachments.push(RawAttachment { uuid, type_uti, skipped });
            }
        }

        // Field 2: ParagraphStyle (embedded message)
        let mut para_style = ParagraphStyle::Body;
        let mut indent: u32 = 0;
        if let Some(para_data) = proto_get_bytes(&run_fields, 2) {
            let para_fields = parse_proto_fields(para_data);
            let style_type = proto_get_varint(&para_fields, 1).unwrap_or(0);
            para_style = match style_type {
                1 => ParagraphStyle::Title,
                2 => ParagraphStyle::Heading,
                3 => ParagraphStyle::Subheading,
                4 => ParagraphStyle::Monospaced,
                100 => ParagraphStyle::BulletList,
                101 => ParagraphStyle::DashedList,
                102 => ParagraphStyle::NumberedList,
                103 => {
                    // Checkbox — field 5 contains todo info
                    let checked = if let Some(todo_data) = proto_get_bytes(&para_fields, 5) {
                        let todo_fields = parse_proto_fields(todo_data);
                        proto_get_varint(&todo_fields, 2).unwrap_or(0) == 1
                    } else {
                        false
                    };
                    ParagraphStyle::Checkbox { checked }
                }
                _ => ParagraphStyle::Body,
            };
            indent = proto_get_varint(&para_fields, 4).unwrap_or(0) as u32;
        }

        // Field 5: font hints (bold/italic encoding)
        let font_hints = proto_get_varint(&run_fields, 5).unwrap_or(0);
        // Field 6: underline
        let underline = proto_get_varint(&run_fields, 6).unwrap_or(0) == 1;
        // Field 7: strikethrough
        let strikethrough = proto_get_varint(&run_fields, 7).unwrap_or(0) == 1;

        let inline_style = InlineStyle {
            bold: font_hints & 1 != 0,
            italic: font_hints & 2 != 0,
            underline,
            strikethrough,
        };

        // Field 9: link URL
        let link = proto_get_string(&run_fields, 9);

        style_runs.push(StyleRun {
            offset: current_pos,
            length,
            paragraph: para_style,
            indent,
            inline_style,
            link,
        });

        current_pos += length;
    }

    // Second pass: find actual U+FFFC positions in the text and pair them
    // with raw_attachments in order. This avoids relying on protobuf run
    // lengths matching Rust's char indexing (they can differ when the text
    // contains emoji or other multi-byte characters).
    let fffc_positions: Vec<usize> = text
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == ATTACHMENT_MARKER)
        .map(|(i, _)| i)
        .collect();

    let mut attachments = Vec::new();
    let mut fffc_idx = 0;
    for raw in &raw_attachments {
        if fffc_idx >= fffc_positions.len() {
            break;
        }
        let actual_pos = fffc_positions[fffc_idx];
        fffc_idx += 1;
        if raw.skipped {
            continue;
        }
        attachments.push(AttachmentPosition {
            position: actual_pos,
            uuid: raw.uuid.clone(),
            type_uti: raw.type_uti.clone(),
        });
    }

    (text, attachments, style_runs)
}

/// Extract plain text and attachments from a (possibly gzipped) ZDATA blob.
pub fn extract_from_zdata(data: &[u8]) -> (String, Vec<AttachmentPosition>) {
    match decompress_gzip(data) {
        Some(decompressed) => extract_note_content_with_attachments(&decompressed),
        None => (String::new(), Vec::new()),
    }
}

/// Extract text, attachments, and style runs from a (possibly gzipped) ZDATA blob.
pub fn extract_from_zdata_styled(data: &[u8]) -> (String, Vec<AttachmentPosition>, Vec<StyleRun>) {
    match decompress_gzip(data) {
        Some(decompressed) => extract_note_full(&decompressed),
        None => (String::new(), Vec::new(), Vec::new()),
    }
}

// ============================================================================
// Cocoa timestamp conversion
// ============================================================================

/// Seconds between Unix epoch (1970-01-01) and Cocoa epoch (2001-01-01).
const COCOA_EPOCH_OFFSET: f64 = 978_307_200.0;

/// Convert a Cocoa Core Data timestamp to a Unix timestamp.
pub fn cocoa_to_unix(cocoa: f64) -> f64 {
    if cocoa == 0.0 {
        return 0.0;
    }
    cocoa + COCOA_EPOCH_OFFSET
}

// ============================================================================
// Filename sanitisation
// ============================================================================

/// Sanitize a string for use as a filename.
pub fn sanitize_filename(name: &str) -> String {
    if name.is_empty() {
        return "Untitled".to_string();
    }

    let sanitized: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control())
        .collect();

    // Collapse whitespace
    let sanitized: String = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");

    let sanitized = sanitized.trim().to_string();

    if sanitized.is_empty() {
        return "Untitled".to_string();
    }

    if sanitized.len() > 200 {
        sanitized[..200].to_string()
    } else {
        sanitized
    }
}

// ============================================================================
// Queries shared with export
// ============================================================================

/// Get notes in a folder, trying multiple join-table schemas.
pub fn get_notes_in_folder(
    conn: &Connection,
    folder_pk: i64,
) -> Result<Vec<NoteRow>, String> {
    // Check if ZMERGEABLEDATA column exists (not present in all DB versions)
    let has_mergeable = conn
        .prepare("SELECT ZMERGEABLEDATA FROM ZICNOTEDATA LIMIT 0")
        .is_ok();
    let data_col = if has_mergeable {
        "COALESCE(d.ZDATA, d.ZMERGEABLEDATA)"
    } else {
        "d.ZDATA"
    };

    let queries = [
        format!(
            "SELECT c.Z_PK, c.ZTITLE1, c.ZSNIPPET, c.ZMODIFICATIONDATE1, \
             {data_col}, c.ZIDENTIFIER \
             FROM Z_12NOTES rel \
             JOIN ZICNOTEDATA d ON d.ZNOTE = rel.Z_9NOTES \
             JOIN ZICCLOUDSYNCINGOBJECT c ON c.Z_PK = rel.Z_9NOTES \
             WHERE rel.Z_12FOLDERS = ?1"
        ),
        format!(
            "SELECT c.Z_PK, c.ZTITLE1, c.ZSNIPPET, c.ZMODIFICATIONDATE1, \
             {data_col}, c.ZIDENTIFIER \
             FROM ZICCLOUDSYNCINGOBJECT c \
             JOIN ZICNOTEDATA d ON d.ZNOTE = c.Z_PK \
             WHERE c.ZFOLDER = ?1"
        ),
        format!(
            "SELECT c.Z_PK, c.ZTITLE1, c.ZSNIPPET, c.ZMODIFICATIONDATE1, \
             {data_col}, c.ZIDENTIFIER \
             FROM Z_11NOTES rel \
             JOIN ZICNOTEDATA d ON d.ZNOTE = rel.Z_8NOTES \
             JOIN ZICCLOUDSYNCINGOBJECT c ON c.Z_PK = rel.Z_8NOTES \
             WHERE rel.Z_11FOLDERS = ?1"
        ),
    ];

    for query in &queries {
        if let Ok(mut stmt) = conn.prepare(query) {
            let rows: Result<Vec<NoteRow>, _> = stmt
                .query_map([folder_pk], |row| {
                    Ok(NoteRow {
                        pk: row.get(0)?,
                        title: row.get(1)?,
                        snippet: row.get(2)?,
                        modified: row.get(3)?,
                        data: row.get(4)?,
                        identifier: row.get(5)?,
                    })
                })
                .map(|rows| rows.filter_map(|r| r.ok()).collect());

            if let Ok(notes) = rows {
                if !notes.is_empty() {
                    return Ok(notes);
                }
            }
        }
    }

    // Dynamic table discovery fallback
    if let Ok(mut tbl_stmt) = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'Z_%NOTES%'",
    ) {
        let tables: Vec<String> = tbl_stmt
            .query_map([], |row| row.get(0))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|r| r.ok())
            .collect();

        for table in tables {
            if let Ok(mut info_stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) {
                let columns: Vec<String> = info_stmt
                    .query_map([], |row| row.get::<_, String>(1))
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.ok())
                    .collect();

                let folder_col = columns.iter().find(|c| c.to_uppercase().contains("FOLDER"));
                let note_col = columns.iter().find(|c| {
                    c.to_uppercase().contains("NOTE") && !c.to_uppercase().contains("FOLDER")
                });

                if let (Some(fc), Some(nc)) = (folder_col, note_col) {
                    let sql = format!(
                        "SELECT c.Z_PK, c.ZTITLE1, c.ZSNIPPET, c.ZMODIFICATIONDATE1, \
                         {data_col}, c.ZIDENTIFIER \
                         FROM {table} rel \
                         JOIN ZICNOTEDATA d ON d.ZNOTE = rel.{nc} \
                         JOIN ZICCLOUDSYNCINGOBJECT c ON c.Z_PK = rel.{nc} \
                         WHERE rel.{fc} = ?1"
                    );
                    if let Ok(mut stmt) = conn.prepare(&sql) {
                        let rows: Result<Vec<NoteRow>, _> = stmt
                            .query_map([folder_pk], |row| {
                                Ok(NoteRow {
                                    pk: row.get(0)?,
                                    title: row.get(1)?,
                                    snippet: row.get(2)?,
                                    modified: row.get(3)?,
                                    data: row.get(4)?,
                                    identifier: row.get(5)?,
                                })
                            })
                            .map(|rows| rows.filter_map(|r| r.ok()).collect());

                        if let Ok(notes) = rows {
                            if !notes.is_empty() {
                                return Ok(notes);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(Vec::new())
}

/// A row from the notes query.
#[derive(Debug)]
#[allow(dead_code)]
pub struct NoteRow {
    pub pk: i64,
    pub title: Option<String>,
    pub snippet: Option<String>,
    pub modified: Option<f64>,
    pub data: Option<Vec<u8>>,
    pub identifier: Option<String>,
}

// ============================================================================
// Debug / diagnostics
// ============================================================================

/// Generate debug info for a note, showing protobuf attachments, DB lookups,
/// and file search results. Returns a multi-line string.
pub fn debug_note(note_path: &str) -> String {
    let pk: i64 = match note_path.strip_prefix("apple-notes://note/") {
        Some(pk_str) => match pk_str.parse() {
            Ok(pk) => pk,
            Err(_) => return format!("Invalid note ID: {note_path}"),
        },
        None => return format!("Invalid note path: {note_path}"),
    };

    let conn = match open_db_cached() {
        Ok(c) => c,
        Err(e) => return format!("DB error: {e}"),
    };

    let mut out = String::new();
    out.push_str(&format!("=== Debug for note PK={pk} ===\n\n"));

    // 1. Read raw ZDATA
    let result = conn.query_row(
        "SELECT ZDATA FROM ZICNOTEDATA WHERE ZNOTE = ?1",
        [pk],
        |row| row.get::<_, Option<Vec<u8>>>(0),
    );

    let data = match result {
        Ok(Some(data)) => {
            out.push_str(&format!("ZDATA: {} bytes\n", data.len()));
            data
        }
        Ok(None) => {
            out.push_str("ZDATA: NULL (no content)\n");
            return out;
        }
        Err(e) => {
            out.push_str(&format!("ZDATA query error: {e}\n"));
            return out;
        }
    };

    let decompressed = match decompress_gzip(&data) {
        Some(d) => {
            out.push_str(&format!("Decompressed: {} bytes\n\n", d.len()));
            d
        }
        None => {
            out.push_str("Decompression FAILED\n");
            return out;
        }
    };

    // 2. Parse protobuf
    let (text, attachments, style_runs) = extract_note_full(&decompressed);
    out.push_str(&format!("Text length: {} chars\n", text.len()));

    // Show U+FFFC positions
    let fffc_positions: Vec<usize> = text
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == ATTACHMENT_MARKER)
        .map(|(i, _)| i)
        .collect();
    out.push_str(&format!("U+FFFC markers in text: {} at positions {:?}\n", fffc_positions.len(), fffc_positions));
    out.push_str(&format!("Attachments from protobuf: {}\n", attachments.len()));

    // Show ALL attribute runs that have attachments (including skipped types)
    {
        let doc_fields = parse_proto_fields(&decompressed);
        if let Some(note_data) = proto_get_bytes(&doc_fields, 2) {
            let note_fields = parse_proto_fields(note_data);
            if let Some(content_data) = proto_get_bytes(&note_fields, 3) {
                let content_fields = parse_proto_fields(content_data);
                let attr_runs = proto_get_all_bytes(&content_fields, 5);
                let mut pos: usize = 0;
                let mut skipped = Vec::new();
                for run_data in attr_runs {
                    let run_fields = parse_proto_fields(run_data);
                    let length = proto_get_varint(&run_fields, 1).unwrap_or(1) as usize;
                    if let Some(att_data) = proto_get_bytes(&run_fields, 12) {
                        let att_fields = parse_proto_fields(att_data);
                        let uuid = proto_get_string(&att_fields, 1).unwrap_or_default();
                        let type_uti = proto_get_string(&att_fields, 2).unwrap_or_default();
                        if SKIP_ATTACHMENT_TYPES.contains(&type_uti.as_str()) {
                            skipped.push(format!("pos={pos} uti={type_uti} uuid={uuid}"));
                        }
                    }
                    pos += length;
                }
                if !skipped.is_empty() {
                    out.push_str(&format!("Skipped attachments: {}\n", skipped.len()));
                    for s in &skipped {
                        out.push_str(&format!("  {s}\n"));
                    }
                }
                // Show unmatched FFFC (no attachment at all in protobuf)
                let att_positions: std::collections::HashSet<usize> =
                    attachments.iter().map(|a| a.position).collect();
                let skipped_positions: std::collections::HashSet<usize> =
                    skipped.iter().filter_map(|s| {
                        s.strip_prefix("pos=")?.split_whitespace().next()?.parse().ok()
                    }).collect();
                let unmatched: Vec<usize> = fffc_positions.iter()
                    .filter(|p| !att_positions.contains(p) && !skipped_positions.contains(p))
                    .copied()
                    .collect();
                if !unmatched.is_empty() {
                    out.push_str(&format!("Unmatched FFFC (no attachment in protobuf): {:?}\n", unmatched));
                }
            }
        }
    }

    // Show style runs for FFFC positions
    out.push_str("\nStyle runs at FFFC positions:\n");
    for &fpos in &fffc_positions {
        if let Some(run) = style_runs.iter().find(|r| fpos >= r.offset && fpos < r.offset + r.length) {
            out.push_str(&format!("  pos={fpos}: para={:?} indent={} bold={} italic={}\n",
                run.paragraph, run.indent, run.inline_style.bold, run.inline_style.italic));
        }
    }
    out.push('\n');

    if attachments.is_empty() && fffc_positions.is_empty() {
        out.push_str("No attachments found in protobuf ZDATA.\n\n");

        // Check ZMERGEABLEDATA
        let merge_result = conn.query_row(
            "SELECT ZMERGEABLEDATA FROM ZICNOTEDATA WHERE ZNOTE = ?1",
            [pk],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        );
        match merge_result {
            Ok(Some(mdata)) => {
                out.push_str(&format!("ZMERGEABLEDATA: {} bytes (present)\n", mdata.len()));
                // Try decompressing and parsing
                if let Some(md) = decompress_gzip(&mdata) {
                    out.push_str(&format!("ZMERGEABLEDATA decompressed: {} bytes\n", md.len()));
                    let md_fields = parse_proto_fields(&md);
                    out.push_str("ZMERGEABLEDATA top-level fields: ");
                    for f in &md_fields {
                        out.push_str(&format!("f{}(wt{},{len}b) ", f.field_number, f.wire_type, len = f.data.len()));
                    }
                    out.push('\n');
                } else {
                    out.push_str("ZMERGEABLEDATA: could not decompress\n");
                }
            }
            Ok(None) => out.push_str("ZMERGEABLEDATA: NULL\n"),
            Err(e) => out.push_str(&format!("ZMERGEABLEDATA: column error ({e})\n")),
        }
        out.push('\n');

        // --- Dump the note's own row ---
        out.push_str(&format!("--- Note row (Z_PK={pk}) ---\n"));
        // Get all column names
        let columns: Vec<String> = {
            let mut cols = Vec::new();
            if let Ok(mut col_stmt) = conn.prepare("PRAGMA table_info(ZICCLOUDSYNCINGOBJECT)") {
                if let Ok(rows) = col_stmt.query_map([], |row| row.get::<_, String>(1)) {
                    for r in rows.flatten() {
                        cols.push(r);
                    }
                }
            }
            cols
        };

        if !columns.is_empty() {
            // Read the note row and show all non-null columns
            let col_list = columns.join(", ");
            let sql = format!("SELECT {col_list} FROM ZICCLOUDSYNCINGOBJECT WHERE Z_PK = ?1");
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(row_result) = stmt.query_row([pk], |row| {
                    let mut vals = Vec::new();
                    for (i, col) in columns.iter().enumerate() {
                        if let Ok(Some(v)) = row.get::<_, Option<String>>(i) {
                            vals.push(format!("{col}=\"{v}\""));
                        } else if let Ok(Some(v)) = row.get::<_, Option<i64>>(i) {
                            vals.push(format!("{col}={v}"));
                        } else if let Ok(Some(v)) = row.get::<_, Option<f64>>(i) {
                            vals.push(format!("{col}={v:.1}"));
                        }
                    }
                    Ok(vals)
                }) {
                    for v in &row_result {
                        out.push_str(&format!("  {v}\n"));
                    }
                }
            }
        }
        out.push('\n');

        // --- Search for any rows referencing this note PK ---
        out.push_str(&format!("--- Rows referencing PK={pk} ---\n"));
        // Find all integer columns that might be FKs
        let int_columns: Vec<String> = {
            let mut cols = Vec::new();
            if let Ok(mut col_stmt) = conn.prepare("PRAGMA table_info(ZICCLOUDSYNCINGOBJECT)") {
                if let Ok(rows) = col_stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                }) {
                    for r in rows.flatten() {
                        let (name, typ) = r;
                        if typ.to_uppercase().contains("INT") && name.starts_with('Z') && name != "Z_PK" {
                            cols.push(name);
                        }
                    }
                }
            }
            cols
        };

        for col in &int_columns {
            let sql = format!(
                "SELECT Z_PK, ZIDENTIFIER, ZTYPEUTI, ZFILENAME, ZMEDIA \
                 FROM ZICCLOUDSYNCINGOBJECT WHERE {col} = ?1 LIMIT 20"
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map([pk], |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                }) {
                    let results: Vec<_> = rows.filter_map(|r| r.ok()).collect();
                    if !results.is_empty() {
                        out.push_str(&format!("\n  Via {col}: {} rows\n", results.len()));
                        for (zpk, zid, ztype, zfname, zmedia) in &results {
                            out.push_str(&format!("    Z_PK={zpk:?} ZIDENTIFIER={zid:?} ZTYPEUTI={ztype:?} ZFILENAME={zfname:?} ZMEDIA={zmedia:?}\n"));
                        }
                    }
                }
            }
        }

        // Check join tables
        if let Ok(mut stmt) = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'Z_%'"
        ) {
            let tables: Vec<String> = stmt.query_map([], |row| row.get(0))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|r| r.ok())
                .collect();

            for table in &tables {
                // Get columns of this join table
                let cols: Vec<String> = {
                    let mut c = Vec::new();
                    if let Ok(mut info_stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) {
                        if let Ok(rows) = info_stmt.query_map([], |row| row.get::<_, String>(1)) {
                            for r in rows.flatten() {
                                c.push(r);
                            }
                        }
                    }
                    c
                };
                {

                    // Search each column for our PK
                    for col in &cols {
                        let sql = format!("SELECT * FROM {table} WHERE {col} = ?1 LIMIT 5");
                        if let Ok(mut sel_stmt) = conn.prepare(&sql) {
                            if let Ok(mut rows) = sel_stmt.query([pk]) {
                                let mut found = Vec::new();
                                while let Ok(Some(row)) = rows.next() {
                                    let mut vals = Vec::new();
                                    for (i, c) in cols.iter().enumerate() {
                                        if let Ok(Some(v)) = row.get::<_, Option<i64>>(i) {
                                            vals.push(format!("{c}={v}"));
                                        }
                                    }
                                    found.push(vals.join(" "));
                                }
                                if !found.is_empty() {
                                    out.push_str(&format!("\n  Join table {table}.{col}: {} rows\n", found.len()));
                                    for f in &found {
                                        out.push_str(&format!("    {f}\n"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        out.push('\n');

        // Dump protobuf structure
        let doc_fields = parse_proto_fields(&decompressed);
        out.push_str("--- ZDATA protobuf structure ---\n");
        out.push_str("Top-level fields: ");
        for f in &doc_fields {
            out.push_str(&format!("f{}(wt{}) ", f.field_number, f.wire_type));
        }
        out.push('\n');

        if let Some(note_data) = proto_get_bytes(&doc_fields, 2) {
            let note_fields = parse_proto_fields(note_data);
            out.push_str("Note (field 2) sub-fields: ");
            for f in &note_fields {
                out.push_str(&format!("f{}(wt{}) ", f.field_number, f.wire_type));
            }
            out.push('\n');

            if let Some(content_data) = proto_get_bytes(&note_fields, 3) {
                let content_fields = parse_proto_fields(content_data);
                out.push_str("Content (field 3) sub-fields: ");
                for f in &content_fields {
                    out.push_str(&format!("f{}(wt{},{len}b) ", f.field_number, f.wire_type, len = f.data.len()));
                }
                out.push('\n');

                // Show field 4 content if present
                if let Some(f4_data) = proto_get_bytes(&content_fields, 4) {
                    let f4_fields = parse_proto_fields(f4_data);
                    out.push_str(&format!("Content field 4 ({} bytes): ", f4_data.len()));
                    for f in &f4_fields {
                        let val = if f.wire_type == 2 {
                            String::from_utf8(f.data.clone()).unwrap_or_else(|_| format!("<{} bytes>", f.data.len()))
                        } else if f.wire_type == 0 {
                            format!("{}", u64::from_le_bytes(f.data[..8].try_into().unwrap_or([0; 8])))
                        } else {
                            format!("<{} bytes>", f.data.len())
                        };
                        out.push_str(&format!("f{}(wt{})={val} ", f.field_number, f.wire_type));
                    }
                    out.push('\n');
                }

                // Show AttributeRun details
                let attr_runs = proto_get_all_bytes(&content_fields, 5);
                out.push_str(&format!("\nAttributeRuns (field 5): {} runs\n", attr_runs.len()));
                for (i, run_data) in attr_runs.iter().enumerate() {
                    let run_fields = parse_proto_fields(run_data);
                    let length = proto_get_varint(&run_fields, 1).unwrap_or(0);
                    let has_att = proto_get_bytes(&run_fields, 12).is_some();
                    let fields_present: Vec<String> = run_fields.iter().map(|f| format!("f{}", f.field_number)).collect();
                    if has_att || i < 5 {
                        out.push_str(&format!("  Run {i}: length={length} fields=[{}] has_attachment={has_att}\n", fields_present.join(",")));
                    }
                }
            }
        }

        return out;
    }

    // 3. Show each attachment's resolution chain
    let notes_base = notes_base_path();
    out.push_str(&format!("Notes base path: {:?}\n\n", notes_base));

    for (i, att) in attachments.iter().enumerate() {
        out.push_str(&format!("--- Attachment {i} ---\n"));
        out.push_str(&format!("  UUID: {}\n", att.uuid));
        out.push_str(&format!("  Type UTI: {}\n", att.type_uti));
        out.push_str(&format!("  Position: {}\n", att.position));

        // DB lookup
        let db_row = conn.query_row(
            "SELECT Z_PK, ZIDENTIFIER, ZMEDIA, ZTYPEUTI, ZFILENAME \
             FROM ZICCLOUDSYNCINGOBJECT WHERE ZIDENTIFIER = ?1",
            [&att.uuid],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        );

        match db_row {
            Ok((zpk, zid, zmedia, ztype, zfname)) => {
                out.push_str(&format!("  DB: Z_PK={zpk:?} ZIDENTIFIER={zid:?} ZMEDIA={zmedia:?} ZTYPEUTI={ztype:?} ZFILENAME={zfname:?}\n"));

                // Follow ZMEDIA chain
                if let Some(media_pk) = zmedia {
                    let media_row = conn.query_row(
                        "SELECT Z_PK, ZIDENTIFIER, ZMEDIA, ZTYPEUTI, ZFILENAME \
                         FROM ZICCLOUDSYNCINGOBJECT WHERE Z_PK = ?1",
                        [media_pk],
                        |row| {
                            Ok((
                                row.get::<_, Option<i64>>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<i64>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, Option<String>>(4)?,
                            ))
                        },
                    );
                    match media_row {
                        Ok((zpk2, zid2, zmedia2, ztype2, zfname2)) => {
                            out.push_str(&format!("  Media: Z_PK={zpk2:?} ZIDENTIFIER={zid2:?} ZMEDIA={zmedia2:?} ZTYPEUTI={ztype2:?} ZFILENAME={zfname2:?}\n"));

                            // Follow second-level ZMEDIA
                            if let Some(media_pk2) = zmedia2 {
                                let m2_row = conn.query_row(
                                    "SELECT Z_PK, ZIDENTIFIER, ZTYPEUTI, ZFILENAME \
                                     FROM ZICCLOUDSYNCINGOBJECT WHERE Z_PK = ?1",
                                    [media_pk2],
                                    |row| {
                                        Ok((
                                            row.get::<_, Option<i64>>(0)?,
                                            row.get::<_, Option<String>>(1)?,
                                            row.get::<_, Option<String>>(2)?,
                                            row.get::<_, Option<String>>(3)?,
                                        ))
                                    },
                                );
                                if let Ok((zpk3, zid3, ztype3, zfname3)) = m2_row {
                                    out.push_str(&format!("  Media2: Z_PK={zpk3:?} ZIDENTIFIER={zid3:?} ZTYPEUTI={ztype3:?} ZFILENAME={zfname3:?}\n"));
                                }
                            }
                        }
                        Err(e) => out.push_str(&format!("  Media lookup FAILED: {e}\n")),
                    }
                }
            }
            Err(e) => {
                out.push_str(&format!("  DB lookup FAILED for UUID '{}': {e}\n", att.uuid));
            }
        }

        // Disk search by attachment UUID
        if let Some(ref base) = notes_base {
            let found = find_attachment_on_disk(base, &att.uuid);
            out.push_str(&format!("  Disk (att UUID): {:?}\n", found));
        }

        // Disk search by media UUID
        let media_uuid: Option<String> = conn
            .query_row(
                "SELECT m.ZIDENTIFIER FROM ZICCLOUDSYNCINGOBJECT a \
                 JOIN ZICCLOUDSYNCINGOBJECT m ON a.ZMEDIA = m.Z_PK \
                 WHERE a.ZIDENTIFIER = ?1",
                [&att.uuid],
                |row| row.get(0),
            )
            .ok();
        if let Some(ref muuid) = media_uuid {
            if let Some(ref base) = notes_base {
                let found = find_attachment_on_disk(base, muuid);
                out.push_str(&format!("  Disk (media UUID {muuid}): {:?}\n", found));

                // List what directories exist for debugging
                let accounts = base.join("Accounts");
                if accounts.is_dir() {
                    if let Ok(entries) = fs::read_dir(&accounts) {
                        for entry in entries.flatten() {
                            let media_dir = entry.path().join("Media").join(muuid);
                            out.push_str(&format!("  Check dir: {} exists={}\n", media_dir.display(), media_dir.exists()));
                            if media_dir.exists() {
                                if let Ok(files) = fs::read_dir(&media_dir) {
                                    for f in files.flatten() {
                                        out.push_str(&format!("    File: {}\n", f.path().display()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        out.push('\n');
    }

    out
}
