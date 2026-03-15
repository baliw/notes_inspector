use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Cache of rendered image lines, keyed by "path:max_width".
    static IMAGE_CACHE: RefCell<HashMap<String, Vec<Line<'static>>>> =
        RefCell::new(HashMap::new());
}

/// Convert markdown text to styled ratatui lines for display.
pub fn markdown_to_lines(content: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();

    let mut bold = false;
    let mut italic = false;
    let code_inline = false;
    let mut in_code_block = false;
    let mut heading_level: Option<u8> = None;
    let mut list_depth: usize = 0;
    let mut in_list_item = false;
    // Deferred blank line from End(Paragraph): emitted before the next block
    // element, but suppressed when a list immediately follows (so paragraphs
    // like bold-styled labels don't get an unwanted gap before their list).
    let mut pending_blank = false;

    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(content, options);

    for event in parser {
        // Consume a deferred paragraph break before new block-level content.
        if pending_blank {
            match &event {
                Event::Start(Tag::List(_)) => {
                    // Suppress blank line — list follows paragraph directly
                    pending_blank = false;
                }
                Event::End(_) => {
                    // Don't consume on End events (e.g. End(Item), End(List))
                }
                _ => {
                    lines.push(Line::raw(""));
                    pending_blank = false;
                }
            }
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    heading_level = Some(level as u8);
                }
                Tag::Strong => bold = true,
                Tag::Emphasis => italic = true,
                Tag::CodeBlock(_) => {
                    push_line(&mut lines, &mut current_spans);
                    in_code_block = true;
                }
                Tag::List(_) => {
                    // Flush any pending text (e.g. parent item text before nested list)
                    push_line(&mut lines, &mut current_spans);
                    list_depth += 1;
                }
                Tag::Item => {
                    in_list_item = true;
                    let indent = "  ".repeat(list_depth.saturating_sub(1));
                    current_spans.push(Span::raw(format!("{indent}• ")));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote(_) => {
                    current_spans.push(Span::styled(
                        "│ ",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                Tag::Link { dest_url, .. } => {
                    current_spans.push(Span::styled(
                        "[",
                        Style::default().fg(Color::Blue),
                    ));
                    let _ = dest_url; // We'll show the link text, not URL
                }
                Tag::Image { dest_url, .. } => {
                    current_spans.push(Span::styled(
                        format!("[Image: {dest_url}]"),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC),
                    ));
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    // Style the heading
                    let level = heading_level.take().unwrap_or(1);
                    let style = match level {
                        1 => Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        2 => Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                        3 => Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                        _ => Style::default().add_modifier(Modifier::BOLD),
                    };

                    let text: String = current_spans.iter().map(|s| s.content.to_string()).collect();
                    let prefix = "#".repeat(level as usize);
                    current_spans = vec![Span::styled(format!("{prefix} {text}"), style)];
                    push_line(&mut lines, &mut current_spans);
                }
                TagEnd::Strong => bold = false,
                TagEnd::Emphasis => italic = false,
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    push_line(&mut lines, &mut current_spans);
                }
                TagEnd::Paragraph => {
                    push_line(&mut lines, &mut current_spans);
                    if list_depth == 0 {
                        // Defer the blank line — it will be suppressed if
                        // a list follows, or emitted before other content.
                        pending_blank = true;
                    }
                }
                TagEnd::Item => {
                    in_list_item = false;
                    push_line(&mut lines, &mut current_spans);
                }
                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                    if list_depth == 0 {
                        lines.push(Line::raw(""));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    push_line(&mut lines, &mut current_spans);
                }
                TagEnd::Link => {
                    current_spans.push(Span::styled(
                        "]",
                        Style::default().fg(Color::Blue),
                    ));
                }
                TagEnd::Image => {}
                _ => {}
            },
            Event::Text(text) => {
                let text = text.to_string();
                if in_code_block {
                    // Show code block lines with a background
                    for code_line in text.lines() {
                        let styled = Span::styled(
                            format!("  {code_line}"),
                            Style::default()
                                .fg(Color::White)
                                .bg(Color::DarkGray),
                        );
                        current_spans.push(styled);
                        push_line(&mut lines, &mut current_spans);
                    }
                } else {
                    let style = build_style(bold, italic, code_inline, heading_level);
                    // Word-wrap if needed
                    wrap_text_into_spans(&text, style, width, &mut current_spans, &mut lines);
                }
            }
            Event::Code(code) => {
                let text = code.to_string();
                current_spans.push(Span::styled(
                    format!("`{text}`"),
                    Style::default().fg(Color::Magenta),
                ));
            }
            Event::SoftBreak => {
                current_spans.push(Span::raw(" "));
            }
            Event::HardBreak => {
                push_line(&mut lines, &mut current_spans);
            }
            Event::Rule => {
                push_line(&mut lines, &mut current_spans);
                let rule = "─".repeat(width.min(80));
                lines.push(Line::styled(rule, Style::default().fg(Color::DarkGray)));
                lines.push(Line::raw(""));
            }
            _ => {}
        }
    }

    // Flush remaining spans
    if !current_spans.is_empty() {
        push_line(&mut lines, &mut current_spans);
    }

    let _ = in_list_item;
    lines
}

fn build_style(bold: bool, italic: bool, code: bool, _heading: Option<u8>) -> Style {
    let mut style = Style::default();
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if code {
        style = style.fg(Color::Magenta);
    }
    style
}

fn push_line(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(spans.drain(..).collect::<Vec<_>>()));
    }
}

fn wrap_text_into_spans(
    text: &str,
    style: Style,
    max_width: usize,
    current_spans: &mut Vec<Span<'static>>,
    lines: &mut Vec<Line<'static>>,
) {
    if max_width == 0 {
        current_spans.push(Span::styled(text.to_string(), style));
        return;
    }

    // Calculate how much width the existing spans on this line already use
    let current_line_width: usize = current_spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum();

    let mut remaining_width = max_width.saturating_sub(current_line_width);

    for word in WordSplitter::new(text) {
        let word_width = unicode_width::UnicodeWidthStr::width(word);

        if word_width <= remaining_width {
            // Word fits on current line
            current_spans.push(Span::styled(word.to_string(), style));
            remaining_width -= word_width;
        } else if word_width <= max_width {
            // Word fits on a new line
            push_line(lines, current_spans);
            current_spans.push(Span::styled(word.trim_start().to_string(), style));
            let trimmed_width = unicode_width::UnicodeWidthStr::width(word.trim_start());
            remaining_width = max_width.saturating_sub(trimmed_width);
        } else {
            // Word is longer than max_width — break it character by character
            if remaining_width == 0 {
                push_line(lines, current_spans);
                remaining_width = max_width;
            }
            let mut chunk = String::new();
            let mut chunk_width = 0;
            for ch in word.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if chunk_width + cw > remaining_width && !chunk.is_empty() {
                    current_spans.push(Span::styled(chunk.clone(), style));
                    push_line(lines, current_spans);
                    chunk.clear();
                    chunk_width = 0;
                    remaining_width = max_width;
                }
                chunk.push(ch);
                chunk_width += cw;
            }
            if !chunk.is_empty() {
                current_spans.push(Span::styled(chunk, style));
                remaining_width = max_width.saturating_sub(chunk_width);
            }
        }
    }
}

/// Split text into words, preserving whitespace as part of each chunk.
/// Yields alternating sequences: "word", " ", "word", " word", etc.
struct WordSplitter<'a> {
    remaining: &'a str,
}

impl<'a> WordSplitter<'a> {
    fn new(text: &'a str) -> Self {
        Self { remaining: text }
    }
}

impl<'a> Iterator for WordSplitter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.remaining.is_empty() {
            return None;
        }

        // Find the end of leading whitespace + next word
        let bytes = self.remaining.as_bytes();
        let mut i = 0;

        // Include leading whitespace
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        // Include non-space characters (the word)
        while i < bytes.len() && bytes[i] != b' ' {
            i += 1;
        }

        if i == 0 {
            // Shouldn't happen, but safety
            i = self.remaining.len();
        }

        let chunk = &self.remaining[..i];
        self.remaining = &self.remaining[i..];
        Some(chunk)
    }
}

/// Convert an image to ANSI art lines (max 24 lines), with caching.
pub fn image_to_ansi_lines(path: &std::path::Path, max_width: usize) -> Vec<Line<'static>> {
    let cache_key = format!("{}:{}", path.display(), max_width);

    // Check cache first
    let cached = IMAGE_CACHE.with(|c| c.borrow().get(&cache_key).cloned());
    if let Some(lines) = cached {
        return lines;
    }

    let lines = render_image(path, max_width);

    // Store in cache
    IMAGE_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, lines.clone());
    });

    lines
}

fn render_image(path: &std::path::Path, max_width: usize) -> Vec<Line<'static>> {
    use fast_image_resize::{images::Image, IntoImageView, ResizeAlg, ResizeOptions, Resizer};

    // Some image formats (HEIC, corrupted files) can panic in the image crate.
    let img_result = std::panic::catch_unwind(|| image::open(path));
    let img = match img_result {
        Ok(Ok(img)) => img,
        Ok(Err(e)) => {
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            return vec![Line::styled(
                format!("[Cannot load image {fname}: {e}]"),
                Style::default().fg(Color::DarkGray),
            )];
        }
        Err(_) => {
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            return vec![Line::styled(
                format!("[Unsupported image format: {fname}]"),
                Style::default().fg(Color::DarkGray),
            )];
        }
    };

    let max_lines: u32 = 24;
    let max_pixel_height = max_lines * 2;
    let max_width = max_width.min(300); // cap to prevent huge allocations

    let (orig_w, orig_h) = (img.width(), img.height());
    if orig_w == 0 || orig_h == 0 {
        return vec![Line::styled("[Empty image]", Style::default().fg(Color::DarkGray))];
    }
    let aspect = orig_w as f64 / orig_h as f64;
    let target_width = (max_pixel_height as f64 * aspect).min(max_width as f64) as u32;
    let target_height = max_pixel_height.min((target_width as f64 / aspect) as u32);
    let target_height = target_height & !1; // make even
    let target_width = target_width.max(1);
    let target_height = target_height.max(2);

    // Use fast_image_resize with SIMD-accelerated bilinear resize
    let src = img.to_rgb8();
    let pixel_type = match src.pixel_type() {
        Some(pt) => pt,
        None => {
            return vec![Line::styled(
                "[Unsupported pixel format]",
                Style::default().fg(Color::DarkGray),
            )];
        }
    };

    let mut dst = Image::new(target_width, target_height, pixel_type);
    let mut resizer = Resizer::new();
    let options =
        ResizeOptions::new().resize_alg(ResizeAlg::Convolution(fast_image_resize::FilterType::Bilinear));

    if let Err(e) = resizer.resize(&src, &mut dst, Some(&options)) {
        let fname = path.file_name().unwrap_or_default().to_string_lossy();
        return vec![Line::styled(
            format!("[Resize failed {fname}: {e}]"),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let buf = dst.buffer();
    let stride = (target_width * 3) as usize; // 3 bytes per RGB pixel

    let mut lines = Vec::new();
    let mut y = 0u32;
    while y < target_height {
        let mut spans = Vec::new();
        let top_row = (y as usize) * stride;
        let bot_row = if y + 1 < target_height {
            ((y + 1) as usize) * stride
        } else {
            top_row
        };

        for x in 0..target_width as usize {
            let ti = top_row + x * 3;
            let bi = bot_row + x * 3;

            spans.push(Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(buf[ti], buf[ti + 1], buf[ti + 2]))
                    .bg(Color::Rgb(buf[bi], buf[bi + 1], buf[bi + 2])),
            ));
        }
        lines.push(Line::from(spans));
        y += 2;
    }

    lines
}
