//! Markdown → styled blocks, pure and GTK-free so the mapping is unit-tested
//! headless (CI and WSL) — see ADR-0004. The preview pane turns these blocks
//! into GTK text tags.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Inline text with formatting flags; adjacent runs share flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// One rendered block of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Run>),
    Paragraph(Vec<Run>),
    Code(String),
    Quote(Vec<Run>),
    Item(Vec<Run>),
    Rule,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Heading(u8),
    Paragraph,
    Quote,
    Item,
}

/// Parse Markdown into flat blocks the preview pane can style. This is the
/// Linux stand-in for the inline live preview on Windows/Android (ADR-0004):
/// source stays plain Markdown, the pane renders a readable view.
pub fn parse_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current: Option<(Kind, Vec<Run>)> = None;
    let mut code: Option<String> = None;
    let mut bold = 0u32;
    let mut italic = 0u32;
    let mut link: Option<String> = None;

    let options = Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut current, &mut blocks);
                current = Some((Kind::Heading(level as u8), Vec::new()));
            }
            Event::End(TagEnd::Heading(_)) => flush(&mut current, &mut blocks),
            Event::Start(Tag::Paragraph) => {
                if current.is_none() {
                    current = Some((Kind::Paragraph, Vec::new()));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if matches!(
                    current.as_ref().map(|(kind, _)| *kind),
                    Some(Kind::Paragraph)
                ) {
                    flush(&mut current, &mut blocks);
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut current, &mut blocks);
                current = Some((Kind::Quote, Vec::new()));
            }
            Event::End(TagEnd::BlockQuote(_)) => flush(&mut current, &mut blocks),
            Event::Start(Tag::Item) => {
                flush(&mut current, &mut blocks);
                current = Some((Kind::Item, Vec::new()));
            }
            Event::End(TagEnd::Item) => flush(&mut current, &mut blocks),
            Event::Start(Tag::CodeBlock(_)) => {
                flush(&mut current, &mut blocks);
                code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(text) = code.take() {
                    blocks.push(Block::Code(text));
                }
            }
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => link = Some(dest_url.to_string()),
            Event::End(TagEnd::Link) => link = None,
            Event::Rule => {
                flush(&mut current, &mut blocks);
                blocks.push(Block::Rule);
            }
            Event::Text(text) => {
                if let Some(buffer) = code.as_mut() {
                    buffer.push_str(&text);
                } else {
                    push_run(
                        &mut current,
                        &text,
                        bold > 0,
                        italic > 0,
                        false,
                        link.clone(),
                    );
                }
            }
            Event::Code(text) => push_run(
                &mut current,
                &text,
                bold > 0,
                italic > 0,
                true,
                link.clone(),
            ),
            Event::SoftBreak => {
                if let Some(buffer) = code.as_mut() {
                    buffer.push('\n');
                } else {
                    push_run(&mut current, " ", bold > 0, italic > 0, false, link.clone());
                }
            }
            Event::HardBreak => {
                if let Some(buffer) = code.as_mut() {
                    buffer.push('\n');
                } else {
                    push_run(
                        &mut current,
                        "\n",
                        bold > 0,
                        italic > 0,
                        false,
                        link.clone(),
                    );
                }
            }
            _ => {}
        }
    }
    flush(&mut current, &mut blocks);
    blocks
}

fn flush(current: &mut Option<(Kind, Vec<Run>)>, blocks: &mut Vec<Block>) {
    let Some((kind, runs)) = current.take() else {
        return;
    };
    if runs.is_empty() {
        return;
    }
    blocks.push(match kind {
        Kind::Heading(level) => Block::Heading(level, runs),
        Kind::Paragraph => Block::Paragraph(runs),
        Kind::Quote => Block::Quote(runs),
        Kind::Item => Block::Item(runs),
    });
}

fn push_run(
    current: &mut Option<(Kind, Vec<Run>)>,
    text: &str,
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<String>,
) {
    let Some((_, runs)) = current else {
        return;
    };
    if let Some(last) = runs.last_mut()
        && last.bold == bold
        && last.italic == italic
        && last.code == code
        && last.link == link
    {
        last.text.push_str(text);
        return;
    }
    runs.push(Run {
        text: text.to_string(),
        bold,
        italic,
        code,
        link,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Run {
        Run {
            text: text.to_string(),
            ..Run::default()
        }
    }

    fn first_runs(blocks: &[Block]) -> &[Run] {
        match &blocks[0] {
            Block::Heading(_, runs)
            | Block::Paragraph(runs)
            | Block::Quote(runs)
            | Block::Item(runs) => runs,
            other => panic!("expected a run block, got {other:?}"),
        }
    }

    #[test]
    fn heading_levels() {
        let blocks = parse_blocks("## Chapter");
        assert_eq!(blocks, vec![Block::Heading(2, vec![plain("Chapter")])]);
    }

    #[test]
    fn bold_and_italic_runs_merge_with_plain_text() {
        let blocks = parse_blocks("**a** and *b*");
        let runs = first_runs(&blocks);
        assert_eq!(
            runs,
            &[
                Run {
                    bold: true,
                    ..plain("a")
                },
                plain(" and "),
                Run {
                    italic: true,
                    ..plain("b")
                },
            ]
        );
    }

    #[test]
    fn soft_break_becomes_space() {
        let blocks = parse_blocks("a\nb");
        assert_eq!(first_runs(&blocks), &[plain("a b")]);
    }

    #[test]
    fn code_block_is_verbatim() {
        let blocks = parse_blocks("```rust\nlet x = 1;\n```");
        assert_eq!(blocks, vec![Block::Code("let x = 1;\n".to_string())]);
    }

    #[test]
    fn inline_code_run() {
        let blocks = parse_blocks("a `c` d");
        let runs = first_runs(&blocks);
        assert_eq!(
            runs,
            &[
                plain("a "),
                Run {
                    code: true,
                    ..plain("c")
                },
                plain(" d"),
            ]
        );
    }

    #[test]
    fn link_target_is_captured() {
        let blocks = parse_blocks("[site](https://example.com)");
        let runs = first_runs(&blocks);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].link.as_deref(), Some("https://example.com"));
        assert_eq!(runs[0].text, "site");
    }

    #[test]
    fn quote_and_list_items() {
        assert_eq!(
            parse_blocks("> quoted"),
            vec![Block::Quote(vec![plain("quoted")])]
        );
        assert_eq!(
            parse_blocks("- one\n- two"),
            vec![
                Block::Item(vec![plain("one")]),
                Block::Item(vec![plain("two")]),
            ]
        );
    }

    #[test]
    fn rule_is_its_own_block() {
        assert_eq!(parse_blocks("a\n\n---\n\nb")[1], Block::Rule);
    }
}
