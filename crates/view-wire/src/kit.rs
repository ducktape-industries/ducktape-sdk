//! WASM composition for the host's native gpui-kit controls.
//!
//! Guests supply content, identity and actions. Controls (buttons, inputs,
//! toggles) take their appearance from the native theme. Layout and text
//! roles — a title, a caption, a card, a chosen list row — are composed here
//! from the shared palette, so every view reads the same way.

use std::cell::Cell;

use crate as wire;
use crate::{Axis, ButtonContent, ButtonPreset, Length, Node, Role};

pub use design::Palette;
pub use design::{height, radius, spacing, type_scale};

thread_local! {
    static DARK: Cell<bool> = const { Cell::new(false) };
}

/// The appearance the session props carry. A view sets it when its session
/// arrives; every helper below reads it.
pub fn set_dark(dark: bool) {
    DARK.with(|cell| cell.set(dark));
}

pub fn is_dark() -> bool {
    DARK.with(Cell::get)
}

/// The palette of the current appearance.
pub fn palette() -> &'static Palette {
    design::palette(is_dark())
}

pub fn rgba(color: design::Color) -> wire::Rgba {
    wire::Rgba(color)
}

/// Show a host-issued image without copying its changing pixels through WASM.
pub fn image_resource(key: impl Into<String>, resource: impl Into<String>) -> Node {
    use std::hash::{Hash, Hasher};
    let resource = resource.into();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ("host-image", &resource).hash(&mut hash);
    Node::Image {
        key: key.into(),
        hash: hash.finish(),
        data: Some(wire::ImageData::Resource(resource)),
        label: None,
        fit: Some(wire::ContentFit::Contain),
        opacity: None,
        width: Some(Length::Fill),
        height: Some(Length::Fill),
    }
}

// ---------- text roles ----------

pub fn text(key: impl Into<String>, content: impl Into<String>) -> Node {
    Node::Text {
        key: key.into(),
        content: content.into(),
        options: Default::default(),
        size: None,
        color: None,
        font: Default::default(),
        width: None,
        align_x: None,
        heading: None,
        live: None,
    }
}

/// A view's own title row.
pub fn title(key: impl Into<String>, content: impl Into<String>) -> Node {
    heading_level(
        weighted(
            text_size(text(key, content), type_scale::TITLE as f32),
            wire::Weight::Semibold,
        ),
        1,
    )
}

/// A section title inside a view.
pub fn heading(key: impl Into<String>, content: impl Into<String>) -> Node {
    heading_level(
        weighted(
            text_size(text(key, content), type_scale::SECTION as f32),
            wire::Weight::Semibold,
        ),
        2,
    )
}

/// What a title looks like is not what it IS: assistive technology reads the
/// level, so the two builders that draw a heading also say so.
fn heading_level(mut node: Node, level: u8) -> Node {
    if let Node::Text { heading, .. } = &mut node {
        *heading = Some(level);
    }
    node
}

/// Body text with emphasis: a row's name, a message author.
pub fn strong(key: impl Into<String>, content: impl Into<String>) -> Node {
    weighted(text(key, content), wire::Weight::Medium)
}

/// Secondary copy beside body text.
pub fn secondary(key: impl Into<String>, content: impl Into<String>) -> Node {
    colored(
        text_size(text(key, content), type_scale::SECONDARY as f32),
        palette().muted,
    )
}

/// A caption: a timestamp, a count, a hint under a control.
pub fn caption(key: impl Into<String>, content: impl Into<String>) -> Node {
    colored(
        text_size(text(key, content), type_scale::CAPTION as f32),
        palette().muted,
    )
}

/// A form label above its field.
pub fn label(key: impl Into<String>, content: impl Into<String>) -> Node {
    weighted(
        colored(
            text_size(text(key, content), type_scale::SECONDARY as f32),
            palette().muted,
        ),
        wire::Weight::Medium,
    )
}

/// An identifier in the data face: a hash, a height, a path.
pub fn mono(key: impl Into<String>, content: impl Into<String>) -> Node {
    let mut node = text_size(text(key, content), type_scale::MONO as f32);
    let Node::Text { font, .. } = &mut node else {
        unreachable!()
    };
    font.monospace = true;
    node
}

/// A status line in one of the four tones.
pub fn tone_text(key: impl Into<String>, content: impl Into<String>, tone: Tone) -> Node {
    colored(
        text_size(text(key, content), type_scale::SECONDARY as f32),
        tone.color(palette()),
    )
}

pub fn text_size(mut node: Node, size: f32) -> Node {
    let Node::Text { size: value, .. } = &mut node else {
        panic!("text_size requires text")
    };
    *value = Some(size);
    node
}

pub fn text_options(mut node: Node, options: wire::TextOptions) -> Node {
    let Node::Text { options: value, .. } = &mut node else {
        panic!("text_options requires text")
    };
    *value = options;
    node
}

pub fn colored(mut node: Node, color: design::Color) -> Node {
    match &mut node {
        Node::Text { color: value, .. } | Node::RichText { color: value, .. } => {
            *value = Some(rgba(color))
        }
        _ => panic!("colored requires text"),
    }
    node
}

pub fn weighted(mut node: Node, weight: wire::Weight) -> Node {
    let Node::Text { font, .. } = &mut node else {
        panic!("weighted requires text")
    };
    font.weight = weight;
    node
}

/// Text that wraps inside its row instead of pushing siblings out.
pub fn wrapping(mut node: Node) -> Node {
    match &mut node {
        Node::Text { options, width, .. } => {
            options.wrapping = Some(wire::Wrapping::WordOrGlyph);
            *width = Some(Length::Fill);
        }
        Node::RichText { options, width, .. } => {
            options.wrapping = Some(wire::Wrapping::WordOrGlyph);
            *width = Some(Length::Fill);
        }
        _ => panic!("wrapping requires text"),
    }
    node
}

/// A label that keeps one line and its natural width.
pub fn nowrap(mut node: Node) -> Node {
    let Node::Text { options, width, .. } = &mut node else {
        panic!("nowrap requires text")
    };
    options.wrapping = Some(wire::Wrapping::None);
    *width = Some(Length::Shrink);
    node
}

/// Cut `text` to `max_chars` and mark the cut with `…`, which counts toward
/// the bound: the result is never longer than `max_chars`, and a text that
/// already fits comes back untouched. `max_chars == 0` gives an empty string.
///
/// COUNTED IN `char`s, not graphemes and not display width: the wire carries
/// no measured text, so a view that must fit a column picks a count, and a
/// wide or combining run still draws wider than a narrow one.
pub fn ellipsize(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    text.chars().take(max_chars - 1).chain(Some('…')).collect()
}

/// The head of an id, a digest or a key: the first `keep` chars, then `…` when
/// there was more. Unlike [`ellipsize`] the mark is NOT counted, so `keep` is
/// how much of the id stays readable whatever its length.
pub fn short_id(id: &str, keep: usize) -> String {
    let mut head: String = id.chars().take(keep).collect();
    if id.chars().count() > keep {
        head.push('…');
    }
    head
}

/// One line of text cut to a count: what a name, a path or a title reads as
/// in a column too narrow for it. See [`ellipsize`] for what the count means.
pub fn truncated(key: impl Into<String>, content: &str, max_chars: usize) -> Node {
    nowrap(text(key, ellipsize(content, max_chars)))
}

// ---------- tones ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Agent,
}

impl Tone {
    pub fn color(self, p: &Palette) -> design::Color {
        match self {
            Tone::Neutral => p.muted,
            Tone::Accent => p.accent_foreground,
            Tone::Success => p.success,
            Tone::Warning => p.warning,
            Tone::Danger => p.danger,
            Tone::Agent => p.agent,
        }
    }
    pub fn wash(self, p: &Palette) -> design::Color {
        match self {
            Tone::Neutral => p.surface_raised,
            Tone::Accent => p.accent_soft,
            Tone::Success => p.success_soft,
            Tone::Warning => p.warning_soft,
            Tone::Danger => p.danger_soft,
            Tone::Agent => p.agent_soft,
        }
    }
}

// ---------- layout ----------

pub fn row(key: impl Into<String>, children: impl IntoIterator<Item = Node>) -> Node {
    linear(key.into(), Axis::Row, children.into_iter().collect())
}

pub fn column(key: impl Into<String>, children: impl IntoIterator<Item = Node>) -> Node {
    linear(key.into(), Axis::Column, children.into_iter().collect())
}

fn linear(key: String, axis: Axis, children: Vec<Node>) -> Node {
    Node::Linear {
        key,
        axis,
        children,
        spacing: Some(spacing::SM as f32),
        padding: None,
        width: Some(Length::Fill),
        height: None,
        max_width: None,
        clip: false,
        wrap: None,
        align: None,
        background: None,
        border: None,
    }
}

/// A row whose children sit on one centre line: a toolbar, a header.
pub fn centered_row(key: impl Into<String>, children: impl IntoIterator<Item = Node>) -> Node {
    let mut node = row(key, children);
    let Node::Linear { align, .. } = &mut node else {
        unreachable!()
    };
    *align = Some(wire::AlignX::Center);
    node
}

/// A row that wraps its children onto further lines.
pub fn wrapped_row(key: impl Into<String>, children: impl IntoIterator<Item = Node>) -> Node {
    let mut node = row(key, children);
    let Node::Linear { wrap, .. } = &mut node else {
        unreachable!()
    };
    *wrap = Some(wire::Wrap {
        spacing: None,
        align: None,
    });
    node
}

pub fn container(key: impl Into<String>, child: Node) -> Node {
    Node::Container {
        key: key.into(),
        content: Box::new(child),
        shadow: Default::default(),
        max_width: None,
        max_height: None,
        clip: false,
        width: Some(Length::Fill),
        height: None,
        padding: None,
        align_x: None,
        align_y: None,
        background: None,
        border: None,
        snap: None,
    }
}

pub fn space(width: Option<Length>, height: Option<Length>) -> Node {
    Node::Space { width, height }
}

/// A flexible gap that takes what its row or column leaves.
pub fn spacer() -> Node {
    space(Some(Length::Fill), Some(Length::Fill))
}

pub fn gap(pixels: f32) -> Node {
    space(Some(Length::Fixed(pixels)), Some(Length::Fixed(pixels)))
}

/// A hairline between regions.
pub fn divider(key: impl Into<String>) -> Node {
    Node::Rule {
        key: key.into(),
        axis: Axis::Row,
        thickness: 1.,
        color: Some(rgba(palette().border)),
        weak: true,
        radius: None,
        snap: None,
    }
}

pub fn vertical_divider(key: impl Into<String>) -> Node {
    Node::Rule {
        key: key.into(),
        axis: Axis::Column,
        thickness: 1.,
        color: Some(rgba(palette().border)),
        weak: true,
        radius: None,
        snap: None,
    }
}

fn border(color: design::Color, radius: f32) -> wire::Border {
    wire::Border {
        color: Some(rgba(color)),
        width: Some(1.),
        radius: Some([radius; 4]),
    }
}

/// A card: the window's own colour inside a hairline, padded content.
pub fn card(key: impl Into<String>, child: Node) -> Node {
    let p = palette();
    let mut node = container(key, child);
    let Node::Container {
        background,
        border: value,
        padding,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *background = Some(wire::Background::Color(rgba(p.background)));
    *value = Some(border(p.border, design::radius::CARD as f32));
    *padding = Some(wire::Edges::all(spacing::LG as f32));
    node
}

/// A card carrying a tone: a notice, a warning, an error.
pub fn notice(key: impl Into<String>, child: Node, tone: Tone) -> Node {
    let p = palette();
    let mut node = container(key, child);
    let Node::Container {
        background,
        border: value,
        padding,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *background = Some(wire::Background::Color(rgba(tone.wash(p))));
    *value = Some(border(tone.color(p), design::radius::CARD as f32));
    *padding = Some(wire::Edges {
        top: spacing::SM as f32,
        right: spacing::LG as f32,
        bottom: spacing::SM as f32,
        left: spacing::LG as f32,
    });
    node
}

/// A pane beside another: a list, a details rail. Surface-toned, with the
/// hairline the host paints between panes.
pub fn pane(key: impl Into<String>, child: Node, width: Length) -> Node {
    let p = palette();
    let mut node = container(key, child);
    let Node::Container {
        background,
        width: w,
        height,
        clip,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *background = Some(wire::Background::Color(rgba(p.surface)));
    *w = Some(width);
    *height = Some(Length::Fill);
    *clip = true;
    node
}

/// The content column of a screen: fills, clips, standard inset.
pub fn page(key: impl Into<String>, children: impl IntoIterator<Item = Node>) -> Node {
    let mut node = column(key, children);
    let Node::Linear {
        padding,
        spacing,
        height,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *padding = Some(wire::Edges::all(20.));
    *spacing = Some(spacing::LG as f32);
    *height = Some(Length::Fill);
    node
}

/// A page that scrolls, at a reading width.
pub fn reading_page(key: impl Into<String>, children: impl IntoIterator<Item = Node>) -> Node {
    let key = key.into();
    let mut node = page(format!("{key}/content"), children);
    let Node::Linear {
        max_width, height, ..
    } = &mut node
    else {
        unreachable!()
    };
    *max_width = Some(880.);
    *height = None;
    scroll(key, node)
}

/// A key/value row: a muted label of fixed width, then the value.
pub fn kv(key: impl Into<String>, name: impl Into<String>, value: Node) -> Node {
    let key = key.into();
    let mut label = secondary(format!("{key}/label"), name);
    let Node::Text { width, options, .. } = &mut label else {
        unreachable!()
    };
    *width = Some(Length::Fixed(140.));
    options.wrapping = Some(wire::Wrapping::None);
    let mut node = row(key, [label, value]);
    let Node::Linear { align, spacing, .. } = &mut node else {
        unreachable!()
    };
    *align = Some(wire::AlignX::Left);
    *spacing = Some(spacing::LG as f32);
    node
}

/// A key/value pair stacked: the same muted label as [`kv`], over the value,
/// with no column to line up against. What [`kv`] becomes where the label
/// would eat the width — the view chooses, since the wire says nothing about
/// how wide the pair landed.
pub fn kv_stacked(key: impl Into<String>, name: impl Into<String>, value: Node) -> Node {
    let key = key.into();
    let mut label = secondary(format!("{key}/label"), name);
    let Node::Text { options, .. } = &mut label else {
        unreachable!()
    };
    options.wrapping = Some(wire::Wrapping::None);
    spaced(column(key, [label, value]), spacing::XS as f32)
}

/// A small tag: a count, a state, a kind.
pub fn badge(key: impl Into<String>, content: impl Into<String>, tone: Tone) -> Node {
    let p = palette();
    let key = key.into();
    let mut node = container(
        key.clone(),
        nowrap(weighted(
            colored(
                text_size(
                    text(format!("{key}/text"), content),
                    type_scale::CAPTION as f32,
                ),
                tone.color(p),
            ),
            wire::Weight::Medium,
        )),
    );
    let Node::Container {
        background,
        border: value,
        padding,
        width,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *background = Some(wire::Background::Color(rgba(tone.wash(p))));
    *value = Some(wire::Border {
        color: None,
        width: None,
        radius: Some([design::radius::CONTROL as f32; 4]),
    });
    *padding = Some(wire::Edges {
        top: 1.,
        right: spacing::XS as f32,
        bottom: 1.,
        left: spacing::XS as f32,
    });
    *width = Some(Length::Shrink);
    node
}

/// Up to two initials off a name: "Reviewer Bot" → "RB", "ab12cd" → "AB".
pub fn initials(name: &str) -> String {
    let words: Vec<char> = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    let picked: String = if words.len() >= 2 {
        words.into_iter().collect()
    } else {
        name.chars().take(2).collect()
    };
    picked.to_uppercase()
}

/// A round avatar carrying initials.
pub fn avatar(key: impl Into<String>, initials: impl Into<String>, tone: Tone) -> Node {
    let p = palette();
    let key = key.into();
    let mut node = container(
        key.clone(),
        nowrap(weighted(
            colored(
                text_size(text(format!("{key}/text"), initials), 10.),
                tone.color(p),
            ),
            wire::Weight::Semibold,
        )),
    );
    let Node::Container {
        background,
        border: value,
        width,
        height,
        align_x,
        align_y,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *background = Some(wire::Background::Color(rgba(tone.wash(p))));
    *value = Some(wire::Border {
        color: None,
        width: None,
        radius: Some([design::radius::PILL as f32; 4]),
    });
    *width = Some(Length::Fixed(24.));
    *height = Some(Length::Fixed(24.));
    *align_x = Some(wire::AlignX::Center);
    *align_y = Some(wire::AlignY::Center);
    node
}

/// What an empty list says: a title and the way forward, centred.
pub fn empty_state(
    key: impl Into<String>,
    title: impl Into<String>,
    detail: impl Into<String>,
) -> Node {
    empty_body(key.into(), title.into(), detail.into(), None)
}

/// An empty state a view can act on: the same block with the way out under
/// the detail — a retry, a create, a link.
pub fn empty_state_action(
    key: impl Into<String>,
    title: impl Into<String>,
    detail: impl Into<String>,
    action: Node,
) -> Node {
    empty_body(key.into(), title.into(), detail.into(), Some(action))
}

fn empty_body(key: String, title: String, detail: String, action: Option<Node>) -> Node {
    let mut node = column(
        key.clone(),
        [
            weighted(
                text_size(
                    text(format!("{key}/title"), title),
                    type_scale::SECTION as f32,
                ),
                wire::Weight::Medium,
            ),
            wrapping(secondary(format!("{key}/detail"), detail)),
        ]
        .into_iter()
        .chain(action),
    );
    let Node::Linear {
        padding,
        max_width,
        align,
        spacing: gap,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *padding = Some(wire::Edges::all(spacing::XL as f32));
    *max_width = Some(420.);
    *align = Some(wire::AlignX::Left);
    *gap = Some(spacing::XXS as f32);
    node
}

/// A form field: label over the control, a hint under it.
pub fn field(key: impl Into<String>, name: impl Into<String>, control: Node) -> Node {
    let key = key.into();
    spaced(
        column(key.clone(), [label(format!("{key}/label"), name), control]),
        spacing::XS as f32,
    )
}

/// A row of section switches: each is a tab, and the chosen one is checked
/// (painted) and selected (announced).
pub fn tabs(
    key: impl Into<String>,
    choices: impl IntoIterator<Item = (String, String, bool, Option<u32>)>,
) -> Node {
    let key = key.into();
    let buttons = choices.into_iter().map(|(id, name, chosen, on_press)| {
        let mut button = button(format!("{key}/{id}"), name, on_press, ButtonPreset::Subtle);
        let Node::Button {
            role,
            checked,
            selected,
            ..
        } = &mut button
        else {
            unreachable!()
        };
        *role = Some(Role::Tab);
        *checked = Some(chosen);
        *selected = Some(chosen);
        button
    });
    spaced(row(key.clone(), buttons), 2.)
}

/// A row in a list pane: the whole row presses, and the chosen one is
/// checked so the native kit paints its selection.
pub fn list_row(key: impl Into<String>, child: Node, chosen: bool, on_press: Option<u32>) -> Node {
    let mut button = button_child(key, child, on_press, ButtonPreset::Subtle);
    let Node::Button {
        checked,
        width,
        padding,
        ..
    } = &mut button
    else {
        unreachable!()
    };
    *checked = Some(chosen);
    *width = Some(Length::Fill);
    *padding = Some(wire::Edges {
        top: spacing::XXS as f32,
        right: spacing::SM as f32,
        bottom: spacing::XXS as f32,
        left: spacing::SM as f32,
    });
    button
}

// ---------- controls ----------

pub fn button(
    key: impl Into<String>,
    label: impl Into<String>,
    on_press: Option<u32>,
    preset: ButtonPreset,
) -> Node {
    control(
        key.into(),
        ButtonContent::Label(label.into()),
        on_press,
        preset,
    )
}

pub fn button_child(
    key: impl Into<String>,
    child: Node,
    on_press: Option<u32>,
    preset: ButtonPreset,
) -> Node {
    control(
        key.into(),
        ButtonContent::Child(Box::new(child)),
        on_press,
        preset,
    )
}

fn control(
    key: String,
    content: ButtonContent,
    on_press: Option<u32>,
    preset: ButtonPreset,
) -> Node {
    let label = match &content {
        ButtonContent::Label(label) => Some(label.clone()),
        ButtonContent::Child(_) => None,
    };
    Node::Button {
        key,
        content,
        on_press,
        label,
        checked: None,
        expanded: None,
        selected: None,
        role: None,
        description: None,
        width: None,
        height: None,
        padding: None,
        style: wire::ButtonStyle {
            preset,
            ..Default::default()
        },
    }
}

pub fn input(
    key: impl Into<String>,
    placeholder: impl Into<String>,
    value: impl Into<String>,
    on_input: u32,
    on_submit: Option<u32>,
) -> Node {
    let placeholder = placeholder.into();
    Node::Input {
        key: key.into(),
        options: wire::InputOptions {
            label: placeholder.clone(),
            ..Default::default()
        },
        placeholder,
        value: value.into(),
        on_input,
        on_submit,
        width: Some(Length::Fill),
        secure: false,
        style: Default::default(),
    }
}

pub fn scroll(key: impl Into<String>, child: Node) -> Node {
    Node::Scroll {
        key: key.into(),
        content: Box::new(child),
        direction: wire::ScrollDirection::Vertical,
        width: Some(Length::Fill),
        height: Some(Length::Fill),
        on_scroll: None,
        virtual_rows: false,
        bar_hidden: false,
        bar_width: None,
        bar_margin: None,
        scroller_width: None,
        bar_spacing: None,
        anchor_x: Default::default(),
        anchor_y: Default::default(),
        auto_scroll: false,
        background: None,
        border: None,
    }
}

/// Apply layout dimensions to a composition node; input height remains native.
pub fn sized(mut node: Node, width: Option<Length>, height: Option<Length>) -> Node {
    match &mut node {
        Node::Linear {
            width: w,
            height: h,
            ..
        }
        | Node::Container {
            width: w,
            height: h,
            ..
        }
        | Node::Button {
            width: w,
            height: h,
            ..
        }
        | Node::Scroll {
            width: w,
            height: h,
            ..
        }
        | Node::Svg {
            width: w,
            height: h,
            ..
        }
        | Node::Image {
            width: w,
            height: h,
            ..
        }
        | Node::Space {
            width: w,
            height: h,
        } => {
            *w = width;
            *h = height;
        }
        Node::Text {
            width: w, options, ..
        } => {
            *w = width;
            options.height = height;
        }
        Node::Input { width: w, .. } => {
            assert!(
                height.is_none(),
                "input height belongs to the native control"
            );
            *w = width;
        }
        _ => panic!("sized requires a layout, text, button, image or input"),
    }
    node
}

pub fn padded(mut node: Node, padding: wire::Edges) -> Node {
    match &mut node {
        Node::Linear { padding: value, .. }
        | Node::Container { padding: value, .. }
        | Node::Button { padding: value, .. } => *value = Some(padding),
        Node::Input { options, .. } => options.padding = Some(padding),
        _ => panic!("padded requires a layout, button or input"),
    }
    node
}

pub fn spaced(mut node: Node, spacing: f32) -> Node {
    let Node::Linear { spacing: value, .. } = &mut node else {
        panic!("spaced requires a row or column")
    };
    *value = Some(spacing);
    node
}

/// Cross-axis alignment of a row or column.
pub fn aligned(mut node: Node, align: wire::AlignX) -> Node {
    let Node::Linear { align: value, .. } = &mut node else {
        panic!("aligned requires a row or column")
    };
    *value = Some(align);
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_defer_appearance_to_the_native_kit_and_keep_routes() {
        let node = button("save", "Save", Some(7), ButtonPreset::Primary);
        let Node::Button {
            style, on_press, ..
        } = node
        else {
            panic!("button")
        };
        assert_eq!(on_press, Some(7));
        assert_eq!(style, wire::ButtonStyle::default());
        let Node::Input {
            options,
            style,
            on_input,
            ..
        } = input("name", "Name", "draft", 9, None)
        else {
            panic!("input")
        };
        assert_eq!(on_input, 9);
        assert_eq!(options.label, "Name");
        assert_eq!(*style, wire::InputStyle::default());
    }

    #[test]
    fn text_roles_follow_the_appearance() {
        set_dark(false);
        let Node::Text { color, size, .. } = caption("c", "now") else {
            panic!("text")
        };
        assert_eq!(color, Some(rgba(design::LIGHT.muted)));
        assert_eq!(size, Some(type_scale::CAPTION as f32));
        set_dark(true);
        let Node::Text { color, .. } = caption("c", "now") else {
            panic!("text")
        };
        assert_eq!(color, Some(rgba(design::DARK.muted)));
        set_dark(false);
    }

    #[test]
    fn a_list_row_is_a_full_width_subtle_button_that_reports_its_choice() {
        let Node::Button {
            checked,
            width,
            style,
            on_press,
            ..
        } = list_row("r", text("t", "Row"), true, Some(3))
        else {
            panic!("button")
        };
        assert_eq!(checked, Some(true));
        assert_eq!(width, Some(Length::Fill));
        assert_eq!(style.preset, ButtonPreset::Subtle);
        assert_eq!(on_press, Some(3));
    }

    #[test]
    fn a_short_id_keeps_its_head_and_marks_only_a_cut() {
        for (id, keep, want) in [
            ("0123456789", 8, "01234567…"),
            ("01234567", 8, "01234567"),
            ("abc", 8, "abc"),
            ("오리테이프", 2, "오리…"),
            ("abc", 0, "…"),
            ("", 0, ""),
        ] {
            assert_eq!(short_id(id, keep), want, "short_id({id:?}, {keep})");
        }
    }

    #[test]
    fn ellipsize_cuts_on_a_char_boundary_and_counts_its_own_mark() {
        let emoji = "🦆🦆🦆";
        for (text, max, want) in [
            ("short", 10, "short"),
            ("exact", 5, "exact"),
            ("truncate me", 5, "trun…"),
            ("", 0, ""),
            ("a", 0, ""),
            ("ab", 1, "…"),
            ("a", 1, "a"),
            ("오리테이프", 3, "오리…"),
            ("오리테이프", 5, "오리테이프"),
            (emoji, 2, "🦆…"),
            (emoji, 1, "…"),
        ] {
            let got = ellipsize(text, max);
            assert_eq!(got, want, "ellipsize({text:?}, {max})");
            assert!(
                got.chars().count() <= max,
                "the mark counts toward the bound"
            );
        }
        let Node::Text {
            content, options, ..
        } = truncated("t", "truncate me", 5)
        else {
            panic!("text")
        };
        assert_eq!(content, "trun…");
        assert_eq!(options.wrapping, Some(wire::Wrapping::None));
    }

    #[test]
    fn a_stacked_pair_is_the_kv_label_over_its_value_with_no_column() {
        let node = kv_stacked("k", "Height", text("k/value", "42"));
        let Node::Linear {
            axis,
            children,
            spacing: gap,
            ..
        } = node
        else {
            panic!("column")
        };
        assert_eq!(axis, Axis::Column);
        assert_eq!(gap, Some(spacing::XS as f32));
        let [label, value] = &children[..] else {
            panic!("label over value")
        };
        let Node::Text {
            key,
            content,
            width,
            size,
            ..
        } = label
        else {
            panic!("text")
        };
        assert_eq!((key.as_str(), content.as_str()), ("k/label", "Height"));
        assert_eq!(*size, Some(type_scale::SECONDARY as f32));
        assert_eq!(*width, None, "a stacked label claims no column");
        assert!(matches!(value, Node::Text { content, .. } if content == "42"));
    }

    #[test]
    fn an_empty_state_carries_its_action_last_and_is_otherwise_unchanged() {
        let plain = empty_state("e", "Nothing here", "Add one to begin.");
        let acting = empty_state_action(
            "e",
            "Nothing here",
            "Add one to begin.",
            button("e/retry", "Retry", Some(1), ButtonPreset::Subtle),
        );
        let (
            Node::Linear {
                children: plain, ..
            },
            Node::Linear {
                children: acting, ..
            },
        ) = (plain, acting)
        else {
            panic!("column")
        };
        assert_eq!(plain.len(), 2);
        assert_eq!(acting.len(), 3);
        assert_eq!(plain[..], acting[..2]);
        assert!(matches!(&acting[2], Node::Button { key, .. } if key == "e/retry"));
    }

    #[test]
    fn the_tokens_draw_what_the_literals_drew() {
        let edges = |top, right, bottom, left| {
            Some(wire::Edges {
                top,
                right,
                bottom,
                left,
            })
        };
        let Node::Linear { spacing, .. } = row("r", []) else {
            panic!("row")
        };
        assert_eq!(spacing, Some(8.));
        let Node::Container { padding, .. } = card("c", text("t", "x")) else {
            panic!("container")
        };
        assert_eq!(padding, edges(12., 12., 12., 12.));
        let Node::Container { padding, .. } = notice("n", text("t", "x"), Tone::Warning) else {
            panic!("container")
        };
        assert_eq!(padding, edges(8., 12., 8., 12.));
        let Node::Container { padding, .. } = badge("b", "3", Tone::Neutral) else {
            panic!("container")
        };
        assert_eq!(padding, edges(1., 6., 1., 6.));
        let Node::Button { padding, .. } = list_row("r", text("t", "Row"), false, None) else {
            panic!("button")
        };
        assert_eq!(padding, edges(4., 8., 4., 8.));
        let Node::Linear {
            padding, spacing, ..
        } = page("p", [])
        else {
            panic!("column")
        };
        assert_eq!((padding, spacing), (edges(20., 20., 20., 20.), Some(12.)));
        let Node::Linear { spacing, .. } = kv("k", "Name", text("v", "x")) else {
            panic!("row")
        };
        assert_eq!(spacing, Some(12.));
        let Node::Linear { spacing, .. } = field("f", "Name", text("v", "x")) else {
            panic!("column")
        };
        assert_eq!(spacing, Some(6.));
        let Node::Linear {
            padding, spacing, ..
        } = empty_state("e", "Nothing", "Add one.")
        else {
            panic!("column")
        };
        assert_eq!((padding, spacing), (edges(24., 24., 24., 24.), Some(4.)));
    }

    #[test]
    fn a_card_paints_the_window_colour_inside_a_hairline() {
        set_dark(false);
        let Node::Container {
            background, border, ..
        } = card("k", text("t", "x"))
        else {
            panic!("container")
        };
        assert_eq!(
            background,
            Some(wire::Background::Color(rgba(design::LIGHT.background)))
        );
        assert_eq!(border.unwrap().color, Some(rgba(design::LIGHT.border)));
    }
}
