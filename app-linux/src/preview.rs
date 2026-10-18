//! Preview pane: `spans::parse_blocks` output mapped onto a read-only
//! `GtkTextView` with registered text tags. Local images render inline as
//! anchored `GtkPicture` widgets (the Tauri shell's inline preview does not
//! do this — see docs/PARITY.md).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;

use crate::spans::{Block, Run, parse_blocks};

/// Widest an inline image is drawn; taller images scale down proportionally.
const MAX_IMAGE_WIDTH: f64 = 460.0;
/// Decoded textures are kept for quick re-renders; beyond this the cache is
/// dropped wholesale (re-rendering rebuilds only what is on screen).
const CACHE_LIMIT: usize = 32;

#[derive(Default)]
struct PreviewState {
    /// Folder of the open note; relative image sources resolve against it.
    base: Option<PathBuf>,
    link_color: String,
    /// Decoded textures keyed by path with (mtime_ns, len), so an edited
    /// attachment is picked up without re-decoding on every keystroke.
    cache: HashMap<PathBuf, (u64, u64, gtk::gdk::Texture)>,
}

#[derive(Clone)]
pub struct Preview {
    pub view: gtk::TextView,
    state: Rc<RefCell<PreviewState>>,
}

impl Preview {
    pub fn new() -> Self {
        let view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(18)
            .bottom_margin(18)
            .left_margin(24)
            .right_margin(24)
            .build();
        add_tags(&view.buffer());
        Self {
            view,
            state: Rc::new(RefCell::new(PreviewState::default())),
        }
    }

    /// Point relative image sources at `base` (the open note's folder).
    pub fn set_base(&self, base: Option<PathBuf>) {
        self.state.borrow_mut().base = base;
    }

    /// Recolour links to the current accent (light or dark variant).
    pub fn set_link_color(&self, color: &str) {
        {
            let mut state = self.state.borrow_mut();
            if state.link_color == color {
                return;
            }
            state.link_color = color.to_string();
        }
        if let Some(tag) = self.view.buffer().tag_table().lookup("link") {
            tag.set_foreground(Some(color));
        }
    }

    /// Re-render the whole document; the spike renders plain text with tags
    /// instead of loading widgets, which keeps it fast and simple. Images are
    /// the exception: they become child widgets at a text anchor.
    pub fn render(&self, markdown: &str) {
        let buffer = self.view.buffer();
        buffer.set_text("");
        let mut end = buffer.end_iter();
        for block in parse_blocks(markdown) {
            match block {
                Block::Heading(level, runs) => {
                    let tag = match level {
                        1 => "h1",
                        2 => "h2",
                        _ => "h3",
                    };
                    self.insert_runs(&buffer, &mut end, &runs, &[tag]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Paragraph(runs) => {
                    self.insert_runs(&buffer, &mut end, &runs, &[]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Quote(runs) => {
                    self.insert_runs(&buffer, &mut end, &runs, &["quote"]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Item(runs) => {
                    buffer.insert_with_tags_by_name(&mut end, "•  ", &["item"]);
                    self.insert_runs(&buffer, &mut end, &runs, &["item"]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Code(code) => {
                    buffer.insert_with_tags_by_name(
                        &mut end,
                        code.trim_end_matches('\n'),
                        &["code-block"],
                    );
                    buffer.insert(&mut end, "\n");
                }
                Block::Rule => {
                    buffer.insert_with_tags_by_name(&mut end, "────────", &["rule"]);
                    buffer.insert(&mut end, "\n");
                }
            }
        }
    }

    fn insert_runs(
        &self,
        buffer: &gtk::TextBuffer,
        end: &mut gtk::TextIter,
        runs: &[Run],
        extra: &[&str],
    ) {
        for run in runs {
            if let Some(src) = run.image.clone()
                && self.insert_image(buffer, end, &src)
            {
                continue;
            }
            let mut tags: Vec<&str> = extra.to_vec();
            if run.bold {
                tags.push("bold");
            }
            if run.italic {
                tags.push("italic");
            }
            if run.code {
                tags.push("inline-code");
            }
            if run.link.is_some() {
                tags.push("link");
            }
            buffer.insert_with_tags_by_name(end, &run.text, &tags);
        }
    }

    /// Draw `src` inline when it is a local file we can decode; return false
    /// so the caller falls back to the alt text (remote URLs, missing files,
    /// undecodable data).
    fn insert_image(&self, buffer: &gtk::TextBuffer, end: &mut gtk::TextIter, src: &str) -> bool {
        if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
            return false;
        }
        let path = {
            let state = self.state.borrow();
            let Some(base) = state.base.as_ref() else {
                return false;
            };
            let src = Path::new(src);
            if src.is_absolute() {
                src.to_path_buf()
            } else {
                base.join(src)
            }
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return false;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_nanos() as u64)
            .unwrap_or(0);
        let size = meta.len();
        let cached = {
            let state = self.state.borrow();
            state
                .cache
                .get(&path)
                .filter(|(cached_mtime, cached_size, _)| {
                    *cached_mtime == mtime && *cached_size == size
                })
                .map(|(_, _, texture)| texture.clone())
        };
        let texture = match cached {
            Some(texture) => texture,
            None => {
                let Ok(bytes) = std::fs::read(&path) else {
                    return false;
                };
                let Ok(texture) = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from(&bytes))
                else {
                    return false;
                };
                let mut state = self.state.borrow_mut();
                if state.cache.len() >= CACHE_LIMIT {
                    state.cache.clear();
                }
                state
                    .cache
                    .insert(path.clone(), (mtime, size, texture.clone()));
                texture
            }
        };
        let width = f64::from(texture.width());
        let height = f64::from(texture.height());
        let (width, height) = if width > MAX_IMAGE_WIDTH {
            (MAX_IMAGE_WIDTH, height * MAX_IMAGE_WIDTH / width)
        } else {
            (width, height)
        };
        let picture = gtk::Picture::builder()
            .paintable(&texture)
            .can_shrink(true)
            .halign(gtk::Align::Start)
            .width_request(width.max(1.0) as i32)
            .height_request(height.max(1.0) as i32)
            .build();
        let anchor = buffer.create_child_anchor(end);
        self.view.add_child_at_anchor(&picture, &anchor);
        true
    }
}

fn add_tags(buffer: &gtk::TextBuffer) {
    let table = buffer.tag_table();
    let push = |tag: gtk::TextTag| {
        table.add(&tag);
    };

    for (name, scale, above, below) in [
        ("h1", 1.7f64, 14i32, 6i32),
        ("h2", 1.4, 10, 4),
        ("h3", 1.2, 8, 3),
    ] {
        let tag = gtk::TextTag::new(Some(name));
        tag.set_weight(700);
        tag.set_scale(scale);
        tag.set_pixels_above_lines(above);
        tag.set_pixels_below_lines(below);
        push(tag);
    }

    let bold = gtk::TextTag::new(Some("bold"));
    bold.set_weight(700);
    push(bold);

    let italic = gtk::TextTag::new(Some("italic"));
    italic.set_style(gtk::pango::Style::Italic);
    push(italic);

    let inline_code = gtk::TextTag::new(Some("inline-code"));
    inline_code.set_family(Some("monospace"));
    inline_code.set_background(Some("rgba(128,128,128,0.15)"));
    push(inline_code);

    let code_block = gtk::TextTag::new(Some("code-block"));
    code_block.set_family(Some("monospace"));
    code_block.set_background(Some("rgba(128,128,128,0.12)"));
    code_block.set_left_margin(12);
    code_block.set_right_margin(12);
    code_block.set_pixels_above_lines(6);
    code_block.set_pixels_below_lines(6);
    push(code_block);

    let quote = gtk::TextTag::new(Some("quote"));
    quote.set_style(gtk::pango::Style::Italic);
    quote.set_left_margin(14);
    quote.set_foreground(Some("rgba(128,128,128,0.95)"));
    push(quote);

    let item = gtk::TextTag::new(Some("item"));
    item.set_left_margin(16);
    push(item);

    let rule = gtk::TextTag::new(Some("rule"));
    rule.set_foreground(Some("rgba(128,128,128,0.6)"));
    push(rule);

    let link = gtk::TextTag::new(Some("link"));
    link.set_foreground(Some("#6d7fe8"));
    link.set_underline(gtk::pango::Underline::Single);
    push(link);
}
