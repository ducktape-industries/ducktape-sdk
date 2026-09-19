//! The one rule for what assistive technology cannot name: the app host and
//! the views' build gate both ask [`accessibility_faults`].

use crate::{ButtonContent, Node};

/// A node assistive technology cannot name or place.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    /// The `key`s from the root down to the node.
    pub path: Vec<String>,
    pub kind: FaultKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaultKind {
    /// A button, a mouse area with a role, or an overlay that nothing names.
    Unnamed,
    /// A mouse area that answers a click without saying what it is.
    NoRole,
    /// A text field, editor, slider, combo box or pick list without a label.
    UnlabeledInput,
    /// An earlier sibling already holds this `key`.
    DuplicateKey,
}

/// Every fault in the tree, depth first. An empty string is no name.
///
/// A mouse area answers a click when `on_press`, `on_release` or
/// `on_double_click` is set; it is named by its `label` or by any non-empty
/// [`Node::Text`] inside it. A button is named by a non-empty label content
/// or `label`, and an overlay by its `label` alone: the text inside a dialog
/// is what it says, not what it is. [`Node::Image`], [`Node::ImageViewer`]
/// and [`Node::Svg`] are skipped: they carry no handler, so whatever makes
/// them interactive is the node that names them.
pub fn accessibility_faults(root: &Node) -> Vec<Fault> {
    let mut faults = Vec::new();
    walk(root, None, &mut faults);
    faults
}

/// The keys above a node, innermost first, kept on the walk's own stack.
struct Path<'a> {
    key: &'a str,
    parent: Option<&'a Path<'a>>,
}

impl Path<'_> {
    fn keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        let mut at = Some(self);
        while let Some(step) = at {
            keys.push(step.key.to_owned());
            at = step.parent;
        }
        keys.reverse();
        keys
    }
}

fn walk(node: &Node, parent: Option<&Path<'_>>, faults: &mut Vec<Fault>) {
    let path = Path {
        key: node.key().unwrap_or_default(),
        parent,
    };
    if let Some(kind) = fault(node) {
        faults.push(Fault {
            path: path.keys(),
            kind,
        });
    }
    let children = node.children();
    for (index, child) in children.iter().enumerate() {
        // Quadratic in a node's children, which `MAX_NODES` bounds.
        if let Some(key) = child.key()
            && children[..index]
                .iter()
                .any(|earlier| earlier.key() == Some(key))
        {
            faults.push(Fault {
                path: Path {
                    key,
                    parent: Some(&path),
                }
                .keys(),
                kind: FaultKind::DuplicateKey,
            });
        }
        walk(child, Some(&path), faults);
    }
}

fn fault(node: &Node) -> Option<FaultKind> {
    match node {
        Node::Input { options, .. } => options
            .label
            .is_empty()
            .then_some(FaultKind::UnlabeledInput),
        Node::Editor { label, .. }
        | Node::Slider { label, .. }
        | Node::ComboBox { label, .. }
        | Node::PickList { label, .. } => (!named(label)).then_some(FaultKind::UnlabeledInput),
        Node::Button { content, label, .. } => {
            let plain = matches!(content, ButtonContent::Label(text) if !text.is_empty());
            (!plain && !named(label)).then_some(FaultKind::Unnamed)
        }
        Node::MouseArea {
            role: None,
            on_press,
            on_release,
            on_double_click,
            ..
        } => (on_press.is_some() || on_release.is_some() || on_double_click.is_some())
            .then_some(FaultKind::NoRole),
        Node::MouseArea { label, content, .. } => {
            (!named(label) && !has_text(content)).then_some(FaultKind::Unnamed)
        }
        Node::Overlay { label, .. } => (!named(label)).then_some(FaultKind::Unnamed),
        _ => None,
    }
}

fn named(label: &Option<String>) -> bool {
    label.as_deref().is_some_and(|label| !label.is_empty())
}

fn has_text(node: &Node) -> bool {
    matches!(node, Node::Text { content, .. } if !content.is_empty())
        || node.children().iter().any(has_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{button, button_child, column, input, text};
    use crate::{AlignX, AlignY, ButtonPreset, Rgba, Role};

    fn overlay(key: &str, label: Option<&str>) -> Node {
        Node::Overlay {
            key: key.into(),
            label: label.map(Into::into),
            padding: 0.0,
            backdrop: Rgba([0.0; 4]),
            align_x: AlignX::Center,
            align_y: AlignY::Center,
            on_dismiss: None,
            children: vec![text(format!("{key}/t"), "Delete this page?")],
        }
    }

    fn area(key: &str, role: Option<Role>, on_press: Option<u32>, content: Node) -> Node {
        Node::MouseArea {
            key: key.into(),
            role,
            label: None,
            expanded: None,
            selected: None,
            checked: None,
            on_press,
            on_release: None,
            on_double_click: None,
            on_right_press: None,
            on_right_release: None,
            on_middle_press: None,
            on_middle_release: None,
            on_enter: None,
            on_exit: None,
            on_move: None,
            on_press_at: None,
            on_scroll: None,
            content: Box::new(content),
        }
    }

    #[test]
    fn each_fault_is_reported_at_its_key_path_and_a_named_tree_has_none() {
        let faulty = column(
            "App",
            [
                input("App/find", "", "", 0, None),
                area("App/open", None, Some(1), text("App/open/t", "Open")),
                button_child("App/gear", Node::empty(), Some(2), ButtonPreset::Subtle),
                text("App/dup", "a"),
                text("App/dup", "b"),
                overlay("App/ask", Some("")),
            ],
        );
        let at = |key: &str| vec!["App".to_owned(), key.to_owned()];
        assert_eq!(
            accessibility_faults(&faulty),
            [
                (at("App/find"), FaultKind::UnlabeledInput),
                (at("App/open"), FaultKind::NoRole),
                (at("App/gear"), FaultKind::Unnamed),
                (at("App/dup"), FaultKind::DuplicateKey),
                (at("App/ask"), FaultKind::Unnamed),
            ]
            .map(|(path, kind)| Fault { path, kind })
        );

        let named = column(
            "App",
            [
                input("App/find", "Search", "", 0, None),
                area(
                    "App/open",
                    Some(Role::Link),
                    Some(1),
                    column("App/open/c", [text("App/open/t", "Open")]),
                ),
                area("App/hover", None, None, Node::empty()),
                button("App/gear", "Settings", Some(2), ButtonPreset::Subtle),
                text("App/a", "a"),
                text("App/b", "b"),
                overlay("App/ask", Some("Delete page")),
            ],
        );
        assert_eq!(accessibility_faults(&named), Vec::<Fault>::new());
    }
}
