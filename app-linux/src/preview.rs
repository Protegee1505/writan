//! Preview pane: `spans::parse_blocks` output mapped onto a read-only
//! `GtkTextView` with registered text tags.

use gtk::prelude::*;

use crate::spans::{Block, Run, parse_blocks};

#[derive(Clone)]
pub struct Preview {
    pub view: gtk::TextView,
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
        Self { view }
    }

    /// Re-render the whole document; the spike renders plain text with tags
    /// instead of loading widgets, which keeps it fast and simple.
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
                    insert_runs(&buffer, &mut end, &runs, &[tag]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Paragraph(runs) => {
                    insert_runs(&buffer, &mut end, &runs, &[]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Quote(runs) => {
                    insert_runs(&buffer, &mut end, &runs, &["quote"]);
                    buffer.insert(&mut end, "\n");
                }
                Block::Item(runs) => {
                    buffer.insert_with_tags_by_name(&mut end, "•  ", &["item"]);
                    insert_runs(&buffer, &mut end, &runs, &["item"]);
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
}

fn insert_runs(buffer: &gtk::TextBuffer, end: &mut gtk::TextIter, runs: &[Run], extra: &[&str]) {
    for run in runs {
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
