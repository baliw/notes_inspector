# Apple Notes Internals

Reference notes on the Apple Notes SQLite database schema and data formats,
derived from the `export_apple_notes_to_obsidian.py` script and macOS Tahoe 26.2.

## Database Location

```
~/Library/Group Containers/group.com.apple.notes/NoteStore.sqlite
```

The database uses SQLite **WAL mode**, so `-wal` and `-shm` journal files may
exist alongside it. To read reliably, copy all three files to a temp directory
first — opening read-only against the live path fails due to WAL requiring
write access to the directory.

## Timestamps

Apple Notes uses **Cocoa Core Data timestamps**: seconds since
`2001-01-01T00:00:00Z`. To convert to Unix epoch:

```
unix_timestamp = cocoa_timestamp + 978307200
```

Relevant columns: `ZCREATIONDATE`, `ZCREATIONDATE1`, `ZMODIFICATIONDATE1`.

## Main Table: `ZICCLOUDSYNCINGOBJECT`

This single table stores folders, notes, and attachments. Row type is
determined by which columns are populated.

### Folders

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZTITLE2` | Folder name (non-null for folders) |
| `ZPARENT` | FK to parent folder's `Z_PK` (for nested folders) |
| `ZFOLDERTYPE` | Folder type identifier |
| `ZMARKEDFORDELETION` | 1 if soft-deleted (column may not exist in all versions) |

### Notes

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZTITLE1` | Note title (non-null for notes) |
| `ZFOLDER` | FK to folder's `Z_PK` |
| `ZSNIPPET` | Plain-text snippet/preview |
| `ZCREATIONDATE` | Cocoa timestamp |
| `ZMODIFICATIONDATE1` | Cocoa timestamp |
| `ZIDENTIFIER` | UUID string |

### Attachments

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZIDENTIFIER` | Attachment UUID |
| `ZTYPEUTI` | UTI string (non-null for attachments) |
| `ZNOTE` | FK to the note's `Z_PK` |
| `ZMEDIA` | FK to the media object's `Z_PK` |
| `ZTITLE` | Attachment title |
| `ZFILENAME` | Original filename |

Media objects (referenced by `ZMEDIA`) also live in this table:

| Column | Purpose |
|--------|---------|
| `ZIDENTIFIER` | Media UUID (used to locate files on disk) |
| `ZFILENAME` | Preferred filename |

## Note Content: `ZICNOTEDATA`

| Column | Purpose |
|--------|---------|
| `ZNOTE` | FK to `ZICCLOUDSYNCINGOBJECT.Z_PK` |
| `ZDATA` | Gzipped protobuf blob containing note body |

## Note-to-Folder Join Tables

The relationship between notes and folders varies across macOS versions.
Multiple query strategies are needed:

1. **Direct FK**: `ZICCLOUDSYNCINGOBJECT.ZFOLDER = folder_pk`
2. **Join table `Z_12NOTES`**: columns `Z_12FOLDERS`, `Z_9NOTES`
3. **Join table `Z_11NOTES`**: columns `Z_11FOLDERS`, `Z_8NOTES`
4. **Dynamic discovery**: query `sqlite_master` for tables matching
   `Z_%NOTES%` and inspect their columns for `FOLDER`/`NOTE` patterns.

## Protobuf Structure (ZDATA after gunzip)

```
Document (root message)
  └─ field 2: Note (embedded message)
       └─ field 3: Content (embedded message)
            ├─ field 2: string — the full note text (UTF-8)
            └─ field 5: repeated AttributeRun (embedded message)
                 ├─ field 1: varint — character length of this run
                 ├─ field 2: ParagraphStyle (embedded message, optional)
                 │    ├─ field 1: varint — style type
                 │    │    0=body, 1=title, 2=heading, 3=subheading,
                 │    │    4=monospaced, 100=bullet list, 101=dashed list,
                 │    │    102=numbered list, 103=checkbox
                 │    ├─ field 4: varint — indent level (0-based)
                 │    └─ field 5: Todo (embedded message, optional, for checkboxes)
                 │         ├─ field 1: string — todo UUID
                 │         └─ field 2: varint — checked (0/1)
                 ├─ field 5: varint — font hints (1=bold, 2=italic, 3=bold+italic)
                 ├─ field 6: varint — underline (1=underlined)
                 ├─ field 7: varint — strikethrough (1=strikethrough)
                 ├─ field 9: string — hyperlink URL (optional)
                 └─ field 12: AttachmentInfo (embedded message, optional)
                      ├─ field 1: string — attachment UUID
                      └─ field 2: string — type UTI
```

### Attachment Marker

The Unicode character `U+FFFC` (Object Replacement Character) appears in the
note text at positions where attachments are embedded. The `AttributeRun`
entries (field 5) track the character offset of each run; when a run has
field 12 (AttachmentInfo), the character at that offset is `U+FFFC` and should
be replaced with the attachment content.

### Attachment Types to Skip

These UTIs represent inline text formatting, not file attachments:
- `com.apple.notes.inlinetextattachment.hashtag`
- `com.apple.notes.inlinetextattachment.mention`

## Attachment Files on Disk

Attachments are stored under the Notes container:

```
~/Library/Group Containers/group.com.apple.notes/
  └─ Accounts/
       └─ {account_uuid}/
            └─ Media/
                 └─ {media_uuid}/
                      └─ {subfolder}/
                           └─ actual_file.ext
```

The directory structure is nested — a recursive search (max depth ~3) from
`Media/{media_uuid}/` is needed to find the actual file.

Fallback locations:
- `Media/{media_uuid}/` (direct, less common)
- `FallbackImages/{media_uuid}/` (preview images)

## Export to Obsidian

When exporting to Obsidian format:
- Folder hierarchy → subdirectories
- Notes → `.md` files (sanitized filenames, max 200 chars)
- Attachments → `_attachments/` folder with unique filenames
- Image attachments → `![[_attachments/file.ext]]` (embedded)
- Other attachments → `[[_attachments/file.ext]]` (linked)
- Remaining `U+FFFC` markers (unfound attachments) are stripped
- File modification times set to note's `ZMODIFICATIONDATE1`
- Filename sanitization: strip `<>:"/\|?*` and control chars
- Duplicate filenames get `_1`, `_2`, etc. suffixes
