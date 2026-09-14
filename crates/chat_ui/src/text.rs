use std::ops::Range;
use std::sync::Arc;

use chat::Mention;
use gpui::{
    AnyElement, App, ElementId, Entity, FontStyle, FontWeight, HighlightStyle, InteractiveText,
    IntoElement, SharedString, StrikethroughStyle, StyledText, UnderlineStyle, Window,
};
use person::PersonRegistry;
use theme::ActiveTheme;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Highlight {
    Code,
    InlineCode(bool),
    Highlight(HighlightStyle),
    Mention,
}

impl From<HighlightStyle> for Highlight {
    fn from(style: HighlightStyle) -> Self {
        Self::Highlight(style)
    }
}

#[derive(Default)]
pub struct RenderedText {
    pub text: SharedString,
    pub highlights: Vec<(Range<usize>, Highlight)>,
    pub link_ranges: Vec<Range<usize>>,
    pub link_urls: Arc<[String]>,
}

impl RenderedText {
    pub fn new(
        content: &str,
        mentions: &[Mention],
        persons: &Entity<PersonRegistry>,
        markdown: bool,
        cx: &App,
    ) -> Self {
        Self::render(content, mentions, markdown, |mention| {
            format!("@{}", persons.read(cx).get(&mention.public_key, cx).name())
        })
    }

    fn render(
        content: &str,
        mentions: &[Mention],
        markdown: bool,
        resolve_mention: impl Fn(&Mention) -> String,
    ) -> Self {
        let mut text = String::new();
        let mut highlights = Vec::new();
        let mut link_ranges = Vec::new();
        let mut link_urls = Vec::new();

        render_text_mut(
            content,
            mentions,
            &mut text,
            &mut highlights,
            &mut link_ranges,
            &mut link_urls,
            markdown,
            resolve_mention,
        );

        RenderedText {
            text: SharedString::from(text),
            link_urls: link_urls.into(),
            link_ranges,
            highlights,
        }
    }

    pub fn element(&self, id: ElementId, window: &Window, cx: &App) -> AnyElement {
        let code_background = cx.theme().elevated_surface_background;
        let color = cx.theme().text_accent;
        let code_font = if cfg!(target_os = "macos") {
            "Menlo"
        } else if cfg!(target_os = "windows") {
            "Consolas"
        } else {
            "monospace"
        };

        InteractiveText::new(
            id,
            StyledText::new(self.text.clone())
                .with_default_highlights(
                    &window.text_style(),
                    self.highlights.iter().map(|(range, highlight)| {
                        (
                            range.clone(),
                            match highlight {
                                Highlight::Code => HighlightStyle {
                                    background_color: Some(code_background),
                                    ..Default::default()
                                },
                                Highlight::InlineCode(link) => {
                                    if *link {
                                        HighlightStyle {
                                            background_color: Some(code_background),
                                            underline: Some(UnderlineStyle {
                                                thickness: 1.0.into(),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        }
                                    } else {
                                        HighlightStyle {
                                            background_color: Some(code_background),
                                            ..Default::default()
                                        }
                                    }
                                }
                                Highlight::Mention => HighlightStyle {
                                    color: Some(color),
                                    underline: Some(UnderlineStyle {
                                        thickness: 1.0.into(),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                Highlight::Highlight(highlight) => *highlight,
                            },
                        )
                    }),
                )
                .with_font_family_overrides(self.highlights.iter().filter_map(
                    |(range, highlight)| {
                        matches!(highlight, Highlight::Code | Highlight::InlineCode(_))
                            .then(|| (range.clone(), code_font.into()))
                    },
                )),
        )
        .on_click(self.link_ranges.clone(), {
            let link_urls = self.link_urls.clone();
            move |ix, _, cx| {
                let url = &link_urls[ix];
                if is_web_url(url) {
                    cx.open_url(url);
                }
            }
        })
        .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_text_mut(
    block: &str,
    mut mentions: &[Mention],
    text: &mut String,
    highlights: &mut Vec<(Range<usize>, Highlight)>,
    link_ranges: &mut Vec<Range<usize>>,
    link_urls: &mut Vec<String>,
    markdown: bool,
    resolve_mention: impl Fn(&Mention) -> String,
) {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut bold_depth = 0;
    let mut italic_depth = 0;
    let mut strikethrough_depth = 0;
    let mut link_url = None;
    let mut list_stack = Vec::new();

    let mut code_block = false;
    let events: Box<dyn Iterator<Item = (Event<'_>, Range<usize>)> + '_> = if markdown {
        Box::new(Parser::new_ext(block, Options::ENABLE_STRIKETHROUGH).into_offset_iter())
    } else {
        Box::new(std::iter::once((Event::Text(block.into()), 0..block.len())))
    };

    for (event, source_range) in events {
        let prev_len = text.len();

        match event {
            Event::Text(t) => {
                if code_block {
                    text.push_str(t.as_ref());
                    highlights.push((prev_len..text.len(), Highlight::Code));
                    continue;
                }
                // Source offsets differ from decoded text after escapes and entities.
                // Locate each original mention token in the decoded event instead.

                let t_str = t.as_ref();
                let mut last_processed = 0;

                while let Some(mention) = mentions.first() {
                    if mention.range.start >= source_range.end {
                        break;
                    }
                    mentions = &mentions[1..];
                    if mention.range.start < source_range.start
                        || mention.range.end > source_range.end
                    {
                        continue;
                    }
                    let Some(token) = block.get(mention.range.clone()) else {
                        continue;
                    };
                    let Some(offset) = t_str[last_processed..].find(token) else {
                        continue;
                    };
                    let mention_start_in_text = last_processed + offset;
                    let mention_end_in_text = mention_start_in_text + token.len();

                    // Add text before this mention
                    if mention_start_in_text > last_processed {
                        let before_mention = &t_str[last_processed..mention_start_in_text];
                        process_text_segment(
                            before_mention,
                            bold_depth,
                            italic_depth,
                            strikethrough_depth,
                            link_url.clone(),
                            text,
                            highlights,
                            link_ranges,
                            link_urls,
                        );
                    }

                    // Process the mention replacement
                    let replacement_text = resolve_mention(mention);

                    let replacement_start = text.len();
                    text.push_str(&replacement_text);
                    let replacement_end = text.len();

                    highlights.push((replacement_start..replacement_end, Highlight::Mention));

                    last_processed = mention_end_in_text;
                }

                // Add any remaining text after the last mention
                if last_processed < t_str.len() {
                    let remaining_text = &t_str[last_processed..];
                    process_text_segment(
                        remaining_text,
                        bold_depth,
                        italic_depth,
                        strikethrough_depth,
                        link_url.clone(),
                        text,
                        highlights,
                        link_ranges,
                        link_urls,
                    );
                }
            }
            Event::Code(t) => {
                text.push_str(t.as_ref());
                let is_link = link_url.is_some();

                if let Some(link_url) = link_url.clone() {
                    link_ranges.push(prev_len..text.len());
                    link_urls.push(link_url);
                }

                highlights.push((prev_len..text.len(), Highlight::InlineCode(is_link)))
            }
            Event::Start(tag) => match tag {
                Tag::Paragraph => new_paragraph(text, &mut list_stack),
                Tag::Heading { .. } => {
                    new_paragraph(text, &mut list_stack);
                    bold_depth += 1;
                }
                Tag::CodeBlock(_kind) => {
                    new_paragraph(text, &mut list_stack);
                    code_block = true;
                }
                Tag::Emphasis => italic_depth += 1,
                Tag::Strong => bold_depth += 1,
                Tag::Strikethrough => strikethrough_depth += 1,
                Tag::Link { dest_url, .. } => {
                    link_url = is_web_url(&dest_url).then(|| dest_url.to_string());
                }
                Tag::List(number) => {
                    list_stack.push((number, false));
                }
                Tag::Item => {
                    let len = list_stack.len();
                    if let Some((list_number, has_content)) = list_stack.last_mut() {
                        *has_content = false;
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push('\n');
                        }
                        for _ in 0..len - 1 {
                            text.push_str("  ");
                        }
                        if let Some(number) = list_number {
                            text.push_str(&format!("{}. ", number));
                            *number += 1;
                            *has_content = false;
                        } else {
                            text.push_str("- ");
                        }
                    }
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::CodeBlock => code_block = false,
                TagEnd::Heading(_) => bold_depth -= 1,
                TagEnd::Emphasis => italic_depth -= 1,
                TagEnd::Strong => bold_depth -= 1,
                TagEnd::Strikethrough => strikethrough_depth -= 1,
                TagEnd::Link => link_url = None,
                TagEnd::List(_) => drop(list_stack.pop()),
                _ => {}
            },
            // HTML is displayed literally; chat messages never execute markup.
            Event::Html(t) | Event::InlineHtml(t) => text.push_str(t.as_ref()),
            Event::Rule => {
                new_paragraph(text, &mut list_stack);
                text.push_str("────────\n");
            }
            Event::HardBreak => text.push('\n'),
            Event::SoftBreak => text.push('\n'),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_text_segment(
    segment: &str,
    bold_depth: i32,
    italic_depth: i32,
    strikethrough_depth: i32,
    link_url: Option<String>,
    text: &mut String,
    highlights: &mut Vec<(Range<usize>, Highlight)>,
    link_ranges: &mut Vec<Range<usize>>,
    link_urls: &mut Vec<String>,
) {
    // Build the style for this segment
    let mut style = HighlightStyle::default();
    if bold_depth > 0 {
        style.font_weight = Some(FontWeight::BOLD);
    }
    if italic_depth > 0 {
        style.font_style = Some(FontStyle::Italic);
    }
    if strikethrough_depth > 0 {
        style.strikethrough = Some(StrikethroughStyle {
            thickness: 1.0.into(),
            ..Default::default()
        });
    }

    // Ranges always refer to the rendered text, including replaced mentions.
    let segment_start = text.len();
    text.push_str(segment);
    let text_end = text.len();

    if let Some(link_url) = link_url {
        // Handle as a markdown link
        link_ranges.push(segment_start..text_end);
        link_urls.push(link_url);
        style.underline = Some(UnderlineStyle {
            thickness: 1.0.into(),
            ..Default::default()
        });

        // Add highlight for the entire linked segment
        if style != HighlightStyle::default() {
            highlights.push((segment_start..text_end, Highlight::Highlight(style)));
        }
    } else {
        // Handle link detection within the segment
        let mut finder = linkify::LinkFinder::new();
        finder.kinds(&[linkify::LinkKind::Url]);
        let mut last_link_pos = 0;

        for link in finder
            .links(segment)
            .filter(|link| is_web_url(link.as_str()))
        {
            let start = link.start();
            let end = link.end();

            // Add non-link text before this link
            if start > last_link_pos {
                let non_link_start = segment_start + last_link_pos;
                let non_link_end = segment_start + start;

                if style != HighlightStyle::default() {
                    highlights.push((non_link_start..non_link_end, Highlight::Highlight(style)));
                }
            }

            // Add the link
            let range = (segment_start + start)..(segment_start + end);
            link_ranges.push(range.clone());
            link_urls.push(link.as_str().to_string());

            // Apply link styling (underline + existing style)
            let mut link_style = style;
            link_style.underline = Some(UnderlineStyle {
                thickness: 1.0.into(),
                ..Default::default()
            });

            highlights.push((range, Highlight::Highlight(link_style)));

            last_link_pos = end;
        }

        // Add any remaining text after the last link
        if last_link_pos < segment.len() {
            let remaining_start = segment_start + last_link_pos;
            let remaining_end = segment_start + segment.len();

            if style != HighlightStyle::default() {
                highlights.push((remaining_start..remaining_end, Highlight::Highlight(style)));
            }
        }
    }
}

fn new_paragraph(text: &mut String, list_stack: &mut [(Option<u64>, bool)]) {
    let mut is_subsequent_paragraph_of_list = false;
    if let Some((_, has_content)) = list_stack.last_mut() {
        if *has_content {
            is_subsequent_paragraph_of_list = true;
        } else {
            *has_content = true;
            return;
        }
    }

    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    for _ in 0..list_stack.len().saturating_sub(1) {
        text.push_str("  ");
    }
    if is_subsequent_paragraph_of_list {
        text.push_str("  ");
    }
}

fn is_web_url(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || url
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::PublicKey;

    fn render(content: &str, markdown: bool) -> RenderedText {
        RenderedText::render(content, &[], markdown, |_| unreachable!())
    }

    fn assert_valid_ranges(rendered: &RenderedText) {
        let mut end = 0;
        for (range, _) in &rendered.highlights {
            assert!(range.start >= end, "overlapping highlights: {range:?}");
            assert!(rendered.text.get(range.clone()).is_some());
            end = range.end;
        }
        assert_eq!(rendered.link_ranges.len(), rendered.link_urls.len());
        for range in &rendered.link_ranges {
            assert!(rendered.text.get(range.clone()).is_some());
        }
    }

    #[test]
    fn renders_nested_inline_styles_and_links() {
        let rendered = render(
            "**bold *both***, *italic*, ~~gone~~ and [site](https://example.com).",
            true,
        );
        assert_eq!(rendered.text.as_ref(), "bold both, italic, gone and site.");
        assert!(rendered.highlights.iter().any(|(range, highlight)| {
            &rendered.text[range.clone()] == "both"
                && matches!(highlight, Highlight::Highlight(style)
                    if style.font_weight == Some(FontWeight::BOLD)
                    && style.font_style == Some(FontStyle::Italic))
        }));
        assert_eq!(&rendered.text[rendered.link_ranges[0].clone()], "site");
        assert_eq!(rendered.link_urls.as_ref(), &["https://example.com"]);
        assert_valid_ranges(&rendered);
    }

    #[test]
    fn code_is_literal_and_not_linkified() {
        let rendered = render(
            "`**inline**`\n\n```rust\nlet x = \"https://example.com\";\n  **literal**\n```\n\nafter",
            true,
        );
        assert_eq!(
            rendered.text.as_ref(),
            "**inline**\n\nlet x = \"https://example.com\";\n  **literal**\n\nafter"
        );
        assert!(rendered.link_ranges.is_empty());
        assert!(
            rendered
                .highlights
                .iter()
                .any(|(_, h)| matches!(h, Highlight::Code))
        );
        assert!(
            rendered
                .highlights
                .iter()
                .any(|(_, h)| matches!(h, Highlight::InlineCode(false)))
        );
        assert_valid_ranges(&rendered);
    }

    #[test]
    fn preserves_plain_text_syntax_and_whitespace() {
        let source = "**bold** &amp; `code`\n\n- item\n[site](https://example.com)\n  ";
        let rendered = render(source, false);
        assert_eq!(rendered.text.as_ref(), source);
        assert_eq!(rendered.link_urls.as_ref(), &["https://example.com"]);
        assert_valid_ranges(&rendered);
    }

    #[test]
    fn renders_headings_paragraphs_and_nested_lists() {
        let rendered = render(
            "# Heading\n\nfirst\nline\n\n3. three\n4. four\n   - nested\n\nafter",
            true,
        );
        assert_eq!(
            rendered.text.as_ref(),
            "Heading\n\nfirst\nline\n3. three\n4. four\n  - nested\n\nafter"
        );
        assert_valid_ranges(&rendered);
    }

    #[test]
    fn mentions_after_code_and_entities_have_valid_output_offsets() {
        let token = "nostr:npub1placeholder";
        let source = format!("`{token}` &amp; é **{token} https://example.com** {token} fin");
        let mentions: Vec<_> = source
            .match_indices(token)
            .map(|(start, _)| {
                Mention::new(
                    PublicKey::from_slice(&[1; 32]).unwrap(),
                    start..start + token.len(),
                )
            })
            .collect();
        let rendered = RenderedText::render(&source, &mentions, true, |_| "@Zoë 🦀".into());
        assert_eq!(
            rendered.text.as_ref(),
            format!("{token} & é @Zoë 🦀 https://example.com @Zoë 🦀 fin")
        );
        assert_eq!(
            &rendered.text[rendered.link_ranges[0].clone()],
            "https://example.com"
        );
        assert_valid_ranges(&rendered);
        let plain = RenderedText::render(&source, &mentions, false, |_| "@Zoë 🦀".into());
        assert_eq!(plain.text.as_ref(), source.replace(token, "@Zoë 🦀"));
        assert_valid_ranges(&plain);
    }

    #[test]
    fn html_is_literal_and_non_web_links_are_not_clickable() {
        let rendered = render(
            "<b>hello</b> [bad](javascript:alert) [file](file:///etc/passwd) [ok](HTTPS://example.com)",
            true,
        );
        assert_eq!(rendered.text.as_ref(), "<b>hello</b> bad file ok");
        assert_eq!(rendered.link_urls.as_ref(), &["HTTPS://example.com"]);
        assert_valid_ranges(&rendered);
    }
}
