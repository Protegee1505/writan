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
    let tags: &[(&str, &[(&str, &dyn glib::ToValue)])] = &[
        (
            "h1",
            &[
                ("weight", &700i32),
                ("scale", &1.7f64),
                ("pixels-above-lines", &14i32),
                ("pixels-below-lines", &6i32),
            ],
        ),
        (
            "h2",
            &[
                ("weight", &700i32),
                ("scale", &1.4f64),
                ("pixels-above-lines", &10i32),
                ("pixels-below-lines", &4i32),
            ],
        ),
        (
            "h3",
            &[
                ("weight", &700i32),
                ("scale", &1.2f64),
                ("pixels-above-lines", &8i32),
                ("pixels-below-lines", &3i32),
            ],
        ),
        ("bold", &[("weight", &700i32)]),
        ("italic", &[("style", &gtk::pango::Style::Italic)]),
        (
            "inline-code",
            &[
                ("family", &"monospace"),
                ("background", &"rgba(128,128,128,0.15)"),
            ],
        ),
        (
            "code-block",
            &[
                ("family", &"monospace"),
                ("background", &"rgba(128,128,128,0.12)"),
                ("left-margin", &12i32),
                ("right-margin", &12i32),
                ("pixels-above-lines", &6i32),
                ("pixels-below-lines", &6i32),
            ],
        ),
        (
            "quote",
            &[
                ("style", &gtk::pango::Style::Italic),
                ("left-margin", &14i32),
                ("foreground", &"rgba(128,128,128,0.95)"),
            ],
        ),
        ("item", &[("left-margin", &16i32)]),
        ("rule", &[("foreground", &"rgba(128,128,128,0.6)")]),
        (
            "link",
            &[
                ("foreground", &"#6d7fe8"),
                ("underline", &gtk::pango::Underline::Single),
            ],
        ),
    ];
    for (name, properties) in tags {
        buffer.create_tag(Some(name), properties);
    }
}
