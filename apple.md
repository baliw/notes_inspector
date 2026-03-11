# Apple Notes Database Internals

Comprehensive reference for decoding the Apple Notes SQLite database, derived from
reverse-engineering on macOS Tahoe 26.2. This documents every quirk, convention,
and gotcha discovered during development of Notes Inspector.

---

## 1. Database Location & Access

### Path

```
~/Library/Group Containers/group.com.apple.notes/NoteStore.sqlite
```

The same container directory also holds attachment files (see §8).

### WAL Mode

The database uses SQLite **WAL (Write-Ahead Logging)** mode. Three files exist:

| File | Purpose |
|------|---------|
| `NoteStore.sqlite` | Main database |
| `NoteStore.sqlite-wal` | Write-ahead log (recent uncommitted changes) |
| `NoteStore.sqlite-shm` | Shared memory index for WAL |

**Critical**: You cannot open the database read-only in place. WAL mode requires
write access to the directory to replay the journal. If you skip the WAL file,
you get stale data — deleted notes still appear, and recent edits are missing.

### Safe Access Strategy

1. Copy all three files (`*.sqlite`, `*-wal`, `*-shm`) to a temp directory
2. Open the copy in read-write mode
3. Checkpoint the WAL: `PRAGMA wal_checkpoint(TRUNCATE)`
4. Switch journal mode: `PRAGMA journal_mode=DELETE`
5. Set read-only: `PRAGMA query_only=ON`

This consolidates all pending WAL changes into the main file and prevents
further writes.

### Permissions

Reading the database requires **Full Disk Access** for the terminal app.
Without it, `fs::copy` fails with `EPERM` (errno 1). The error message should
direct users to: System Settings → Privacy & Security → Full Disk Access.

### Caching

After the initial copy, subsequent reads can reuse the temp copy without
re-copying. This is significantly faster for operations like note preview
where the database is queried per-note.

---

## 2. Timestamps

Apple Notes uses **Cocoa Core Data timestamps**: floating-point seconds since
`2001-01-01T00:00:00Z` (the Core Data epoch).

```
unix_timestamp = cocoa_timestamp + 978307200
```

Relevant columns:

| Column | Where | Purpose |
|--------|-------|---------|
| `ZCREATIONDATE` | Notes | Creation time |
| `ZCREATIONDATE1` | Notes | Alternative creation time column |
| `ZMODIFICATIONDATE1` | Notes | Last modified time |

Not all columns exist in every macOS version. Always check with
`SELECT col FROM table LIMIT 0` before using.

---

## 3. The Polymorphic Table: `ZICCLOUDSYNCINGOBJECT`

This single table stores **folders**, **notes**, **attachments**, and
**media objects**. Row type is determined by which columns are populated.
There is no explicit type discriminator column.

### Column Availability

Columns vary across macOS versions. Before querying any optional column,
probe it:

```sql
SELECT column_name FROM ZICCLOUDSYNCINGOBJECT LIMIT 0
```

If this returns an error, the column does not exist. Key optional columns:

| Column | Notes |
|--------|-------|
| `ZMARKEDFORDELETION` | Soft-delete flag (may not exist) |
| `ZPARENT` | Folder parent FK (may not exist) |
| `ZFOLDERTYPE` | Folder classification (may not exist) |
| `ZMODIFICATIONDATE1` | Modification timestamp (may not exist) |
| `ZISPINNED` | Note pinned flag (may not exist) |
| `ZMERGEABLEDATA` | Alternative content blob in `ZICNOTEDATA` |

### Identifying Row Types

| Row type | How to identify |
|----------|-----------------|
| Folder | `ZTITLE2 IS NOT NULL` |
| Note | `ZTITLE1 IS NOT NULL` |
| Attachment | `ZTYPEUTI IS NOT NULL` (has a UTI string) |
| Media object | Referenced by another row's `ZMEDIA` FK |

### Folder Columns

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZTITLE2` | Folder name |
| `ZPARENT` | FK to parent folder's `Z_PK` (for nested folders) |
| `ZFOLDERTYPE` | `NULL` or `0` = user folder, `1` = system folder |
| `ZMARKEDFORDELETION` | `1` = soft-deleted (in trash) |

**Quirk**: `ZTITLE2 IS NOT NULL` also matches account containers
("iCloud", "On My Mac") and system folders ("Recently Deleted"). Use
`ZFOLDERTYPE` to filter:

```sql
WHERE ZTITLE2 IS NOT NULL
  AND (ZFOLDERTYPE IS NULL OR ZFOLDERTYPE = 0)
  AND (ZMARKEDFORDELETION IS NULL OR ZMARKEDFORDELETION != 1)
```

Even with filtering, some folders end up empty (e.g., account-level
containers whose children are system folders). Prune folders with zero
notes recursively after building the tree.

### Note Columns

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZTITLE1` | Note title |
| `ZFOLDER` | FK to the containing folder's `Z_PK` |
| `ZSNIPPET` | Plain-text preview snippet |
| `ZCREATIONDATE` | Cocoa timestamp |
| `ZMODIFICATIONDATE1` | Cocoa timestamp |
| `ZIDENTIFIER` | UUID string |
| `ZISPINNED` | `1` if pinned to top of folder |

### Attachment Columns

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZIDENTIFIER` | Attachment UUID (referenced from protobuf) |
| `ZTYPEUTI` | UTI string (e.g., `public.jpeg`, `public.png`) |
| `ZNOTE` | FK to the note's `Z_PK` |
| `ZMEDIA` | FK to the media object's `Z_PK` |
| `ZTITLE` | Attachment title |
| `ZFILENAME` | Original filename (often NULL on attachment rows) |

### Media Object Columns

Media objects are also rows in `ZICCLOUDSYNCINGOBJECT`, referenced by
an attachment's `ZMEDIA` FK:

| Column | Purpose |
|--------|---------|
| `Z_PK` | Primary key |
| `ZIDENTIFIER` | Media UUID (used to locate files on disk) |
| `ZFILENAME` | Preferred filename (e.g., `IMG_7185.jpeg`) |
| `ZMEDIA` | FK to another media object (chained, for some attachment types) |

**Important**: The attachment row's `ZIDENTIFIER` is different from the media
object's `ZIDENTIFIER`. File lookup on disk uses the **media UUID**, not the
attachment UUID.

---

## 4. Note-to-Folder Relationships

The mapping between notes and folders varies across macOS versions. Multiple
strategies must be tried:

### Strategy 1: Direct Foreign Key (most common)

```sql
SELECT * FROM ZICCLOUDSYNCINGOBJECT
WHERE ZTITLE1 IS NOT NULL AND ZFOLDER = ?
```

The `ZFOLDER` column on note rows is an FK to the folder's `Z_PK`.

### Strategy 2: Join Table `Z_12NOTES`

```sql
SELECT Z_9NOTES FROM Z_12NOTES WHERE Z_12FOLDERS = ?
```

Columns: `Z_12FOLDERS` (folder PK), `Z_9NOTES` (note PK).

### Strategy 3: Join Table `Z_11NOTES`

```sql
SELECT Z_8NOTES FROM Z_11NOTES WHERE Z_11FOLDERS = ?
```

Columns: `Z_11FOLDERS` (folder PK), `Z_8NOTES` (note PK).

### Strategy 4: Dynamic Discovery

Query `sqlite_master` for tables matching `Z_%NOTES%` and inspect their
columns for `FOLDER`/`NOTE` naming patterns. The column names encode the
relationship numbers which change between schema versions.

---

## 5. Note Content: `ZICNOTEDATA` Table

| Column | Purpose |
|--------|---------|
| `ZNOTE` | FK to `ZICCLOUDSYNCINGOBJECT.Z_PK` |
| `ZDATA` | Gzipped protobuf blob containing note body |
| `ZMERGEABLEDATA` | Alternative content blob (newer macOS versions) |

**Quirk**: Some notes have `ZDATA = NULL` but `ZMERGEABLEDATA` populated.
Always use `COALESCE(ZDATA, ZMERGEABLEDATA)` when reading. Check if the
`ZMERGEABLEDATA` column exists first.

### Decompression

The `ZDATA` blob is **gzip-compressed**. Check for the gzip magic bytes
(`0x1f 0x8b`) before decompressing. Some blobs may not be gzipped (pass
through as-is).

---

## 6. Protobuf Structure

After gunzipping `ZDATA`, the result is a protobuf message with the
following nested structure:

```
Document (root message)
└─ field 2: Note (length-delimited, embedded message)
     └─ field 3: Content (length-delimited, embedded message)
          ├─ field 2: string — the full note text (UTF-8)
          └─ field 5: repeated AttributeRun (length-delimited, embedded message)
```

### The Note Text (field 2)

A single UTF-8 string containing the entire note text. Lines are separated
by `\n`. Attachment positions are marked with `U+FFFC` (Object Replacement
Character). The text includes the title as the first line.

### AttributeRun (field 5, repeated)

Each AttributeRun describes formatting for a contiguous range of characters.
Runs are ordered and their lengths sum to the total text length.

```
AttributeRun
├─ field 1: varint — character count (length of this run)
├─ field 2: ParagraphStyle (length-delimited, optional)
│    ├─ field 1: varint — style type
│    │    0  = body (default)
│    │    1  = title
│    │    2  = heading
│    │    3  = subheading
│    │    4  = monospaced (code block)
│    │    100 = bullet list
│    │    101 = dashed list
│    │    102 = numbered list
│    │    103 = checkbox (todo)
│    ├─ field 4: varint — indent level (0-based, for nested lists)
│    └─ field 5: Todo (length-delimited, optional, only for style 103)
│         ├─ field 1: string — todo UUID
│         └─ field 2: varint — checked state (0 = unchecked, 1 = checked)
├─ field 5: varint — font hints (bitmask)
│    bit 0 (value 1) = bold
│    bit 1 (value 2) = italic
│    value 3 = bold + italic
├─ field 6: varint — underline (1 = underlined)
├─ field 7: varint — strikethrough (1 = strikethrough)
├─ field 9: string — hyperlink URL (optional, present when text is a link)
└─ field 12: AttachmentInfo (length-delimited, optional)
     ├─ field 1: string — attachment UUID (references ZIDENTIFIER in DB)
     └─ field 2: string — type UTI (e.g., "public.jpeg")
```

### Critical Quirk: Paragraph Style Location

The paragraph style (heading, list type, etc.) is stored on the
**AttributeRun covering the trailing `\n`** that terminates the paragraph,
**not** on the run covering the text characters.

Example: for a heading line "Hello\n", the run covering "Hello" might have
`ParagraphStyle=Body`, while the run covering "\n" has
`ParagraphStyle=Heading`. You must look up the style from the run at the
line's trailing newline position, not the first character.

### Critical Quirk: Run Lengths vs. Rust Char Indexing

Protobuf run lengths **do not always match** Rust's `char` counting. When
the text contains emoji or other multi-byte/multi-codepoint characters,
cumulative run offsets drift from actual character positions. **Never rely
on summing run lengths to compute character positions.** Instead, scan the
actual text string to find marker positions (see §7).

---

## 7. Attachment Handling

### U+FFFC Markers

The Unicode character `U+FFFC` (Object Replacement Character) marks
attachment positions in the note text. Every attachment (including
skipped types like hashtags and mentions) has a corresponding `U+FFFC`
in the text.

### Matching FFFC to Attachments

The protobuf's `AttributeRun` entries with `field 12` (AttachmentInfo)
contain the attachment UUID and UTI. These appear in the same order as
the `U+FFFC` markers in the text.

**Algorithm** (handles skipped types and position drift correctly):

1. Scan the text for all `U+FFFC` positions:
   ```
   fffc_positions = [i for i, ch in enumerate(text) if ch == '\uFFFC']
   ```

2. Collect all `AttachmentInfo` entries from the protobuf in order,
   marking skipped types.

3. Walk both lists in parallel:
   ```
   fffc_idx = 0
   for each raw_attachment in protobuf order:
       if fffc_idx >= len(fffc_positions): break
       actual_position = fffc_positions[fffc_idx]
       fffc_idx += 1
       if raw_attachment is skipped type: continue
       emit attachment at actual_position
   ```

This ensures correct position mapping even when:
- Skipped attachment types (hashtags, mentions) consume FFFC markers
- Protobuf run length sums differ from Rust char indices

### Skipped Attachment Types

These UTIs represent inline text formatting, not file attachments. They
have corresponding `U+FFFC` in the text and `AttachmentInfo` in the
protobuf, but should **not** produce file references:

| UTI | Meaning |
|-----|---------|
| `com.apple.notes.inlinetextattachment.hashtag` | Hashtag (#tag) |
| `com.apple.notes.inlinetextattachment.mention` | Mention (@person) |

When skipped, their `U+FFFC` marker is simply removed from the output text.

### Position-Based Lookup

Use a `HashMap<char_position, replacement_string>` for attachment
substitution. **Do not** use sequential indexing or a counter — skipped
attachment types (hashtags, mentions) cause sequential matching to get
out of sync.

### Consecutive Attachments

When multiple `U+FFFC` markers appear close together (e.g., positions
11, 13, 15 with single characters between them), the characters between
markers can corrupt replacement strings if not handled carefully. Always
ensure a newline separator after each attachment replacement so
intervening characters don't get appended to file paths.

---

## 8. Attachment Files on Disk

### Container Path

```
~/Library/Group Containers/group.com.apple.notes/
```

### Directory Structure

```
Accounts/
└─ {account_uuid}/
     ├─ Media/
     │    └─ {media_uuid}/
     │         └─ {subfolder}/          ← e.g., "1_8FA3C665-C8E1-..."
     │              └─ actual_file.ext  ← e.g., "IMG_7185.jpeg"
     ├─ FallbackImages/
     │    └─ {media_uuid}/
     │         └─ preview_image.ext
     └─ Previews/
          └─ {media_uuid}/
               └─ preview.ext
```

### Lookup Chain

To find the file for an attachment:

1. **Get media UUID**: Query the attachment's `ZMEDIA` FK to get the media
   object, then read its `ZIDENTIFIER`.

   ```sql
   SELECT m.ZIDENTIFIER
   FROM ZICCLOUDSYNCINGOBJECT a
   LEFT JOIN ZICCLOUDSYNCINGOBJECT m ON a.ZMEDIA = m.Z_PK
   WHERE a.ZIDENTIFIER = ?
   ```

2. **Search `Media/{media_uuid}/`**: Recursively (max depth 3) search for
   the first media file. The UUID directory contains a subdirectory with
   a `1_` prefix and another UUID, which contains the actual file.

3. **Fallback — attachment UUID**: If media UUID lookup fails, search
   `Media/{attachment_uuid}/`.

4. **Fallback — deeper media chain**: Some attachments have `media → media`
   chains (the media object itself has a `ZMEDIA` FK to another media object):

   ```sql
   SELECT a.ZIDENTIFIER, m.ZIDENTIFIER, m2.ZIDENTIFIER
   FROM ZICCLOUDSYNCINGOBJECT a
   LEFT JOIN ZICCLOUDSYNCINGOBJECT m ON a.ZMEDIA = m.Z_PK
   LEFT JOIN ZICCLOUDSYNCINGOBJECT m2 ON m.ZMEDIA = m2.Z_PK
   WHERE a.ZIDENTIFIER = ?
   ```

5. **Fallback — `FallbackImages/`**: Search preview images.

6. **Fallback — `Previews/`**: Search preview directory.

### File Discovery Within UUID Directories

The UUID directory contains subdirectories and files. When scanning:

- **Skip**: `.DS_Store`, files with `._` prefix (macOS metadata)
- **Prefer**: Files with media extensions (jpg, jpeg, png, gif, heic, heif,
  tiff, tif, bmp, webp, svg, pdf, mov, mp4, m4v, m4a, mp3, aac, wav)
- **Fallback**: Any other non-metadata file
- **Recurse**: Into subdirectories (max depth 3)

### HEIC Images

Apple Notes commonly stores photos as HEIC (High Efficiency Image Coding).
Obsidian and many image renderers don't support HEIC natively. Use macOS
`sips` to convert:

```bash
sips -s format jpeg -s formatOptions 85 input.heic --out output.jpg
```

---

## 9. Formatting Conversion

### Paragraph Styles to Markdown

| Style Code | Apple Notes | Markdown |
|------------|------------|----------|
| 0 | Body | (no prefix) |
| 1 | Title | `# ` |
| 2 | Heading | `## ` |
| 3 | Subheading | `### ` |
| 4 | Monospaced | Wrap in ``` fences |
| 100 | Bullet list | `- ` |
| 101 | Dashed list | `- ` |
| 102 | Numbered list | `1. `, `2. `, etc. |
| 103 | Checkbox (unchecked) | `- [ ] ` |
| 103 | Checkbox (checked) | `- [x] ` |

### Indent Levels

The `indent` value from `ParagraphStyle.field 4` maps to markdown indentation:
`"  ".repeat(indent)` prepended before the list prefix. This creates nested
lists.

### Inline Styles to Markdown

| Style | Markdown |
|-------|----------|
| Bold | `**text**` |
| Italic | `*text*` |
| Bold + Italic | `***text***` |
| Strikethrough | `~~text~~` |
| Link | `[text](url)` |

### Code Blocks

Monospaced paragraphs (style 4) are fenced with ``` markers. Track
transitions in/out of code blocks. No inline formatting is applied
inside code blocks.

### Numbered List Counter

Track a running counter for numbered lists. Reset to 0 when transitioning
from a non-list paragraph. Increment for each numbered list item. The
counter continues across consecutive numbered items regardless of indent.

### Empty List Items

Apple Notes allows empty list items for spacing. Preserve these — emit the
list prefix (`- `, `1. `, etc.) even when the line content is blank. Only
skip prefixes for empty **non-list** lines (body, heading).

### Dash Lines

Lines consisting entirely of dashes (`-`), em dashes (`—`), or en dashes
(`–`) are converted to markdown horizontal rules: `---`.

### Notes Callout Pattern

A specific pattern in Apple Notes creates an Obsidian info callout:

1. **Opener**: A line containing the word "notes" (case-insensitive) where
   all other characters are dashes, spaces, or em/en dashes.
   Examples: `--- notes ----`, `-- Notes --`, `--- NOTES ---`

2. **Content**: Any lines between the opener and closer

3. **Closer**: A line of 4 or more dashes by itself

This converts to:

```markdown
> [!info]
> content line 1
> content line 2
```

Blank lines within the callout get the `> ` prefix to maintain the
blockquote.

---

## 10. Preview Rendering Quirks

### Hard Breaks for Body Text

Markdown parsers (pulldown-cmark) merge consecutive lines in a paragraph
into a single block (soft break = space). Apple Notes treats each `\n` as
a visible line break. To preserve this, append `  ` (two trailing spaces)
to body text lines — the markdown hard break syntax.

**Do not** add hard breaks to block-level elements:
- Headings (`#`)
- List items (`- `, `* `, `1. `)
- Code fences (` ``` `)
- Horizontal rules (`---`)
- Blockquotes (`>`)
- Blank lines
- Inline image markers

Adding hard breaks to list items breaks pulldown-cmark's list structure
parsing, causing extra newlines and collapsed nested lists.

### Inline Image Markers

For TUI preview, attachments are replaced with `__INLINE_IMAGE__:path`
markers. The preview renderer splits on this marker, checks if the path
exists and is an image, and renders ANSI art.

**Gotcha**: The last `__INLINE_IMAGE__:` in the text may have no trailing
newline (stripped by `trim()`). The path extractor must handle both
`path\nrest` and `path` (no newline, end of string).

### Callout Rendering

Obsidian `> [!info]` callouts are detected in the preview text and
rendered with:
- Colored header line (`│ ℹ  INFO`)
- `│ ` prefix on content lines
- Dark blue background filling the full line width
- `│ ` on blank lines within the callout

---

## 11. Export to Obsidian

### Output Structure

```
output_dir/
├─ _attachments/
│    ├─ unique_filename.jpg
│    └─ ...
├─ Folder Name/
│    ├─ Note Title.md
│    └─ Subfolder/
│         └─ ...
└─ ...
```

### Filename Sanitization

- Strip characters: `< > : " / \ | ? *`
- Strip ASCII control characters (0x00–0x1F)
- Truncate to 200 characters
- Deduplicate with `_1`, `_2`, etc. suffixes

### Attachment Handling in Export

- Image attachments: `![[_attachments/filename.ext]]` (Obsidian embedded image)
- Other attachments: `[[_attachments/filename.ext]]` (Obsidian link)
- HEIC files: converted to JPEG via `sips` during copy
- Position-based `HashMap<char_position, obsidian_link>` for substitution
- Remaining `U+FFFC` markers (unfound attachments) are stripped

### File Metadata

Set exported `.md` file modification times to the note's
`ZMODIFICATIONDATE1` (converted from Cocoa to Unix timestamp) using
`filetime::set_file_mtime`.

### Formatting

The full `apply_markdown_formatting` pipeline is applied:
paragraph styles, inline styles, code blocks, horizontal rules,
callout conversion, attachment embedding — producing valid Obsidian
markdown.

---

## 12. Protobuf Parsing (No Schema)

The protobuf is parsed without a `.proto` schema using raw wire-type
decoding:

### Wire Types

| Type | Meaning | Encoding |
|------|---------|----------|
| 0 | Varint | LEB128 variable-length integer |
| 2 | Length-delimited | Varint length prefix + bytes |

### Parsing Algorithm

1. Read field tag: `(field_number << 3) | wire_type` as a varint
2. Based on wire type:
   - **Varint (0)**: Read LEB128 bytes
   - **Length-delimited (2)**: Read length varint, then that many bytes
3. Collect all `(field_number, value)` pairs
4. For nested messages (length-delimited): recursively parse the bytes

### Helper Functions

- `proto_get_varint(fields, field_num)` → `Option<u64>`
- `proto_get_bytes(fields, field_num)` → `Option<&[u8]>` (first occurrence)
- `proto_get_string(fields, field_num)` → `Option<String>`
- `proto_get_all_bytes(fields, field_num)` → `Vec<&[u8]>` (all occurrences,
  for repeated fields like AttributeRun)

---

## 13. Known Edge Cases & Gotchas

1. **WAL without copy = stale data**: Deleted notes still appear, recent
   edits are missing.

2. **Column existence varies**: Always probe columns before using them.
   Different macOS versions have different schemas.

3. **ZMERGEABLEDATA**: Some notes only have content in this column, not
   `ZDATA`. Use `COALESCE`.

4. **Paragraph style on newline, not text**: The heading/list style is on
   the `\n` run, not the text run. Reading from the wrong position causes
   headings to appear on wrong lines.

5. **Run length drift**: Emoji and multi-byte characters cause protobuf
   cumulative offsets to diverge from Rust `char` indices. Scan actual text
   for `U+FFFC` positions instead of computing from run lengths.

6. **Skipped attachments consume FFFC**: Hashtags and mentions have FFFC
   markers AND protobuf entries, but aren't file attachments. Sequential
   counters break — use position-based HashMap.

7. **Attachment UUID ≠ Media UUID**: Files on disk are under the **media**
   UUID, not the attachment UUID. Must join through `ZMEDIA` FK.

8. **Media chain depth**: Some attachments require following
   `ZMEDIA → ZMEDIA → ZMEDIA` chains (2-3 levels deep).

9. **Permission denied = Full Disk Access**: Error code `EPERM` (1) when
   copying the database means the terminal lacks Full Disk Access.

10. **Empty folders after filtering**: Account containers and system folders
    pass `ZTITLE2 IS NOT NULL` but contain no user notes. Prune recursively.

11. **Consecutive FFFC markers**: Characters between adjacent markers get
    appended to replacement strings. Always add newline separators after
    attachment replacements.

12. **Last attachment path truncated**: `trim()` in post-processing removes
    trailing newlines. The last `__INLINE_IMAGE__:path` may lack a trailing
    `\n`, so the path extractor must handle paths at end-of-string.

13. **HEIC not renderable**: Most image libraries and Obsidian can't display
    HEIC. Convert to JPEG during export with `sips`.

14. **Hard breaks break lists**: Adding `  \n` to list items causes
    pulldown-cmark to misparse nested list structure. Only apply to body text.

15. **Folder nesting depth**: ZPARENT chains can be multiple levels deep.
    Build folder hierarchy iteratively (multiple passes) rather than
    assuming single-level nesting.

16. **Terminal corruption after TUI**: Crossterm's `LeaveAlternateScreen`
    and `disable_raw_mode` are insufficient. Shell out to the `reset`
    command for full terminal restoration, including in the panic hook.
