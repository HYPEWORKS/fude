//! Accessibility: the UI described for screen readers and other assistive technologies,
//! as an AccessKit tree (fude-platform hands it to the OS).
//!
//! Widgets describe themselves from what they drew in the last frame, so bounds are the
//! rectangles on screen, in physical pixels. Text is described as it is on screen: each
//! row of a text field is one or more text runs, and a document describes only the rows
//! in view, so building the tree costs the same at any file size.

use accesskit::{
    Action, Node, NodeId, Rect as AkRect, Role, TextPosition, TextSelection, TreeId, TreeInfo,
    TreeUpdate,
};
use fude_editor::text::{Class, class_of};
use fude_render::Rect;
use fude_text::Chip;
use unicode_segmentation::UnicodeSegmentation;

/// What a node is. Its [`NodeId`] is this, encoded, so a request from an assistive
/// technology can be routed back to the widget it concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Window,
    MenuBar,
    /// A menu's title in the menu bar.
    MenuTitle(usize),
    /// The open menu: the context menu, or one of the menu bar's.
    Menu,
    MenuItem(usize),
    Tabs,
    Tab(usize),
    TabClose(usize),
    Editor,
    FindBar,
    FindButton(usize),
    FindStatus,
    /// A single-line text field.
    Field(Field),
    /// Text run `k` of the document, or of a field.
    Run(Text, usize),
    StatusBar,
    StatusItem(usize),
    Palette,
    PaletteList,
    PaletteItem(usize),
    Prompt,
    PromptMessage,
    PromptButton(usize),
}

/// The single-line text fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Find,
    Replace,
    Palette,
}

/// What holds a text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    Editor,
    Field(Field),
}

const FIELDS: [Field; 3] = [Field::Find, Field::Replace, Field::Palette];

impl Part {
    /// (kind, index): kinds without an index use 0.
    fn code(self) -> (u64, usize) {
        let field = |f: Field| FIELDS.iter().position(|&x| x == f).unwrap_or(0) as u64;
        match self {
            Part::Window => (1, 0),
            Part::MenuBar => (2, 0),
            Part::MenuTitle(i) => (3, i),
            Part::Menu => (4, 0),
            Part::MenuItem(i) => (5, i),
            Part::Tabs => (6, 0),
            Part::Tab(i) => (7, i),
            Part::TabClose(i) => (8, i),
            Part::Editor => (9, 0),
            Part::FindBar => (10, 0),
            Part::FindButton(i) => (11, i),
            Part::FindStatus => (12, 0),
            Part::StatusBar => (13, 0),
            Part::StatusItem(i) => (14, i),
            Part::Palette => (15, 0),
            Part::PaletteList => (16, 0),
            Part::PaletteItem(i) => (17, i),
            Part::Prompt => (18, 0),
            Part::PromptMessage => (19, 0),
            Part::PromptButton(i) => (20, i),
            Part::Field(f) => (21 + field(f), 0),
            Part::Run(Text::Editor, k) => (24, k),
            Part::Run(Text::Field(f), k) => (25 + field(f), k),
        }
    }

    pub fn id(self) -> NodeId {
        let (kind, index) = self.code();
        NodeId((kind << 32) | (index as u64 & 0xFFFF_FFFF))
    }

    /// The part a node id stands for.
    pub fn of(id: NodeId) -> Option<Part> {
        let (kind, i) = (id.0 >> 32, (id.0 & 0xFFFF_FFFF) as usize);
        let field = |k: u64| FIELDS.get(k as usize).copied();
        Some(match kind {
            1 => Part::Window,
            2 => Part::MenuBar,
            3 => Part::MenuTitle(i),
            4 => Part::Menu,
            5 => Part::MenuItem(i),
            6 => Part::Tabs,
            7 => Part::Tab(i),
            8 => Part::TabClose(i),
            9 => Part::Editor,
            10 => Part::FindBar,
            11 => Part::FindButton(i),
            12 => Part::FindStatus,
            13 => Part::StatusBar,
            14 => Part::StatusItem(i),
            15 => Part::Palette,
            16 => Part::PaletteList,
            17 => Part::PaletteItem(i),
            18 => Part::Prompt,
            19 => Part::PromptMessage,
            20 => Part::PromptButton(i),
            21..=23 => Part::Field(field(kind - 21)?),
            24 => Part::Run(Text::Editor, i),
            25..=27 => Part::Run(Text::Field(field(kind - 25)?), i),
            _ => return None,
        })
    }
}

/// The nodes of a tree being built.
#[derive(Default)]
pub struct AccessTree {
    nodes: Vec<(NodeId, Node)>,
}

impl AccessTree {
    pub fn new() -> AccessTree {
        AccessTree::default()
    }

    pub fn push(&mut self, part: Part, node: Node) -> NodeId {
        let id = part.id();
        self.nodes.push((id, node));
        id
    }

    /// A node pushed already, to add to it.
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes
            .iter_mut()
            .rev()
            .find(|(n, _)| *n == id)
            .map(|(_, node)| node)
    }

    /// The update, with [`Part::Window`] (which the caller pushed) as the root.
    ///
    /// AccessKit panics on a tree that isn't one (in the event loop, where fude can't
    /// recover), so it's checked here first: a tree that fails is a bug, which costs the
    /// screen reader everything but the window rather than costing the user their work.
    pub fn finish(mut self, focus: Part) -> TreeUpdate {
        let mut focus = focus.id();
        if let Err(problem) = self.check(focus) {
            debug_assert!(false, "accessibility tree: {problem}");
            eprintln!("fude: accessibility tree: {problem}");
            let window = self
                .nodes
                .into_iter()
                .find(|(id, _)| *id == Part::Window.id());
            let mut window = window.map_or_else(|| Node::new(Role::Window), |(_, node)| node);
            window.clear_children();
            self.nodes = vec![(Part::Window.id(), window)];
            focus = Part::Window.id();
        }
        TreeUpdate {
            nodes: self.nodes,
            tree: Some(TreeInfo {
                root: Part::Window.id(),
                toolkit_name: Some("fude".to_owned()),
                toolkit_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            tree_id: TreeId::ROOT,
            focus,
        }
    }

    /// Whether the nodes make a tree AccessKit accepts: no id twice, the root and the
    /// focus among them, and every other node the child of exactly one.
    fn check(&self, focus: NodeId) -> Result<(), String> {
        let mut ids = std::collections::HashSet::with_capacity(self.nodes.len());
        for (id, _) in &self.nodes {
            if !ids.insert(*id) {
                return Err(format!("{:?} twice", Part::of(*id)));
            }
        }
        let root = Part::Window.id();
        if !ids.contains(&root) {
            return Err("no window".to_owned());
        }
        if !ids.contains(&focus) {
            return Err(format!("focus on {:?}, which isn't there", Part::of(focus)));
        }
        let mut parented = std::collections::HashSet::with_capacity(self.nodes.len());
        for (_, node) in &self.nodes {
            for child in node.children() {
                if !ids.contains(child) || !parented.insert(*child) || *child == root {
                    return Err(format!("{:?} misplaced", Part::of(*child)));
                }
            }
        }
        if parented.len() + 1 != ids.len() {
            return Err("a node without a parent".to_owned());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get(&self, part: Part) -> Option<&Node> {
        let id = part.id();
        self.nodes
            .iter()
            .find(|(n, _)| *n == id)
            .map(|(_, node)| node)
    }
}

/// A rectangle on screen, for a node's bounds.
pub fn bounds(r: Rect) -> AkRect {
    AkRect::new(
        f64::from(r.x),
        f64::from(r.y),
        f64::from(r.x + r.w.max(0)),
        f64::from(r.y + r.h.max(0)),
    )
}

/// A node with a label and bounds.
pub fn labeled(role: Role, label: &str, rect: Rect) -> Node {
    let mut node = Node::new(role);
    node.set_label(label);
    node.set_bounds(bounds(rect));
    node
}

/// A button (or other control) that can be clicked.
pub fn clickable(role: Role, label: &str, rect: Rect) -> Node {
    let mut node = labeled(role, label, rect);
    node.add_action(Action::Click);
    node
}

/// Text that isn't a control. Its text is its name, so a live one is read out when it
/// changes.
pub fn text_node(value: &str, rect: Rect) -> Node {
    let mut node = Node::new(Role::Label);
    node.set_value(value);
    node.set_bounds(bounds(rect));
    node
}

/// Text runs hold at most this many characters, as their word starts are bytes. A longer
/// row is several runs on one line.
const RUN_CHARS: usize = 255;

/// A row of text on screen, to describe as text runs.
pub(crate) struct RowText<'a> {
    /// The row's bytes, from offset `start` of the text.
    pub bytes: &'a [u8],
    pub start: usize,
    /// The line break that ends the row, if it ends a line.
    pub line_break: &'a [u8],
    /// The class of the character before the row, if it continues a line that wraps or
    /// is long (`None` at the start of a line).
    pub before: Option<Class>,
    /// The row's top and height on screen.
    pub y: i32,
    pub h: i32,
}

/// One character of a run: where it is in the text, and what it says.
struct Char<'a> {
    offset: usize,
    len: usize,
    value: &'a str,
    class: Class,
}

/// The text runs a text field was last described with, to map positions in them back
/// to offsets in its text.
#[derive(Default, Debug)]
pub(crate) struct TextRuns {
    /// For each run, the offset of each character's start, then of the run's end.
    pub(crate) runs: Vec<Vec<usize>>,
}

impl TextRuns {
    pub fn clear(&mut self) {
        self.runs.clear();
    }

    /// Describes a row as text runs, returning their ids. `x_of` is the x on screen of
    /// an offset in the row (the row's end included).
    pub fn push_row(
        &mut self,
        tree: &mut AccessTree,
        owner: Text,
        row: &RowText<'_>,
        mut x_of: impl FnMut(usize) -> f32,
    ) -> Vec<NodeId> {
        let chars = characters(row);
        let mut ids = Vec::new();
        if chars.is_empty() {
            // An empty last line: a place for the caret.
            let x = x_of(row.start);
            let k = self.runs.len();
            self.runs.push(vec![row.start]);
            let mut node = Node::new(Role::TextRun);
            node.set_value("");
            node.set_character_lengths(Vec::<u8>::new());
            node.set_character_positions(Vec::<f32>::new());
            node.set_character_widths(Vec::<f32>::new());
            node.set_bounds(AkRect::new(
                f64::from(x),
                f64::from(row.y),
                f64::from(x),
                f64::from(row.y + row.h),
            ));
            ids.push(tree.push(Part::Run(owner, k), node));
            return ids;
        }
        let row_end = row.start + row.bytes.len();
        let mut xs: Vec<f32> = chars.iter().map(|c| x_of(c.offset.min(row_end))).collect();
        xs.push(x_of(row_end));
        let starts = word_starts(&chars, row.before);
        let first_run = self.runs.len();
        let mut from = 0;
        while from < chars.len() {
            let to = split_point(&starts, from, chars.len());
            let k = self.runs.len();
            let run = &chars[from..to];
            let x0 = xs[from];
            let x1 = xs[to].max(x0);
            let mut value = String::new();
            let mut lengths = Vec::with_capacity(run.len());
            let mut positions = Vec::with_capacity(run.len());
            let mut widths = Vec::with_capacity(run.len());
            for (i, c) in run.iter().enumerate() {
                value.push_str(c.value);
                lengths.push(c.value.len() as u8);
                let x = xs[from + i];
                positions.push(x - x0);
                // A line break takes no room (as in Notepad).
                let w = if c.class == Class::LineBreak {
                    0.0
                } else {
                    (xs[from + i + 1] - x).max(0.0)
                };
                widths.push(w);
            }
            let words: Vec<u8> = starts
                .iter()
                .filter(|&&s| s >= from && s < to)
                .map(|&s| (s - from) as u8)
                .collect();
            let mut node = Node::new(Role::TextRun);
            node.set_value(value);
            node.set_character_lengths(lengths);
            node.set_character_positions(positions);
            node.set_character_widths(widths);
            node.set_word_starts(words);
            node.set_bounds(AkRect::new(
                f64::from(x0),
                f64::from(row.y),
                f64::from(x1),
                f64::from(row.y + row.h),
            ));
            if k > first_run {
                node.set_previous_on_line(Part::Run(owner, k - 1).id());
            }
            if to < chars.len() {
                node.set_next_on_line(Part::Run(owner, k + 1).id());
            }
            let mut offsets: Vec<usize> = run.iter().map(|c| c.offset).collect();
            offsets.push(run.last().map_or(row.start, |c| c.offset + c.len));
            self.runs.push(offsets);
            ids.push(tree.push(Part::Run(owner, k), node));
            from = to;
        }
        ids
    }

    /// Where offset `at` is among the runs. Offsets before or after the text described
    /// go to its start or end.
    pub fn position(&self, owner: Text, at: usize) -> Option<TextPosition> {
        let pos = |k: usize, character_index: usize| TextPosition {
            node: Part::Run(owner, k).id(),
            character_index,
        };
        for (k, offsets) in self.runs.iter().enumerate() {
            if at < offsets[0] {
                return Some(pos(k, 0));
            }
            let end = offsets[offsets.len() - 1];
            if at < end {
                return Some(pos(k, offsets.partition_point(|&o| o <= at) - 1));
            }
        }
        let last = self.runs.len().checked_sub(1)?;
        Some(pos(last, self.runs[last].len() - 1))
    }

    /// A selection from `anchor` to `head`.
    pub fn selection(&self, owner: Text, anchor: usize, head: usize) -> Option<TextSelection> {
        Some(TextSelection {
            anchor: self.position(owner, anchor)?,
            focus: self.position(owner, head)?,
        })
    }

    /// The offset of a position in these runs.
    pub fn offset(&self, owner: Text, position: &TextPosition) -> Option<usize> {
        let Some(Part::Run(o, k)) = Part::of(position.node) else {
            return None;
        };
        if o != owner {
            return None;
        }
        self.runs.get(k)?.get(position.character_index).copied()
    }
}

/// A row's characters: grapheme clusters (each byte that isn't UTF-8 on its own), then the
/// line break. Control characters read as their Unicode symbols (␀ for NUL), as they
/// show as chips; bytes that aren't UTF-8 as replacement characters.
fn characters<'a>(row: &RowText<'a>) -> Vec<Char<'a>> {
    let mut out = Vec::with_capacity(row.bytes.len() + 1);
    let mut offset = row.start;
    // Printable ASCII and tabs (most text): a character per byte, no clusters to find.
    if row
        .bytes
        .iter()
        .all(|&b| (b' '..=b'~').contains(&b) || b == b'\t')
        && let Ok(text) = std::str::from_utf8(row.bytes)
    {
        for (i, b) in row.bytes.iter().enumerate() {
            out.push(Char {
                offset: offset + i,
                len: 1,
                value: &text[i..i + 1],
                class: class_of(Some(char::from(*b))),
            });
        }
        offset += row.bytes.len();
    }
    for chunk in row.bytes[offset - row.start..].utf8_chunks() {
        for g in chunk.valid().graphemes(true) {
            // Runs count bytes per character in a byte: split absurdly long clusters.
            let mut rest = g;
            while !rest.is_empty() {
                let mut len = rest.len().min(u8::MAX as usize);
                while !rest.is_char_boundary(len) {
                    len -= 1;
                }
                let piece = &rest[..len];
                let first = piece.chars().next();
                let value = match first.and_then(Chip::for_char) {
                    Some(Chip::Control(c)) => control_symbol(c),
                    _ => piece,
                };
                out.push(Char {
                    offset,
                    len,
                    value,
                    class: class_of(first),
                });
                offset += len;
                rest = &rest[len..];
            }
        }
        for _ in chunk.invalid() {
            out.push(Char {
                offset,
                len: 1,
                value: "\u{FFFD}",
                class: class_of(None),
            });
            offset += 1;
        }
    }
    if !row.line_break.is_empty() {
        out.push(Char {
            offset,
            len: row.line_break.len(),
            value: if row.line_break == b"\r\n" {
                "\r\n"
            } else {
                "\n"
            },
            class: Class::LineBreak,
        });
    }
    out
}

/// The Unicode symbol for a control character (U+2400 onwards), or the replacement
/// character for those without one.
fn control_symbol(c: u8) -> &'static str {
    const C0: [&str; 32] = [
        "␀", "␁", "␂", "␃", "␄", "␅", "␆", "␇", "␈", "␉", "␊", "␋", "␌", "␍", "␎", "␏", "␐", "␑",
        "␒", "␓", "␔", "␕", "␖", "␗", "␘", "␙", "␚", "␛", "␜", "␝", "␞", "␟",
    ];
    match c {
        0..=0x1F => C0[usize::from(c)],
        0x7F => "␡",
        _ => "\u{FFFD}",
    }
}

/// The characters that start words, as Ctrl+Right stops: a word is a run of word or
/// punctuation characters with the spaces after it; a line's indentation is a word of
/// its own.
fn word_starts(chars: &[Char<'_>], before: Option<Class>) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut prev = before;
    for (i, c) in chars.iter().enumerate() {
        let start = match prev {
            None => true,
            Some(p) => !matches!(c.class, Class::Space | Class::LineBreak) && c.class != p,
        };
        if start {
            starts.push(i);
        }
        prev = Some(c.class);
    }
    starts
}

/// Where a run starting at `from` ends: at most [`RUN_CHARS`] characters on, at a word
/// start if one is in the second half.
fn split_point(starts: &[usize], from: usize, len: usize) -> usize {
    let limit = from + RUN_CHARS;
    if len <= limit {
        return len;
    }
    starts
        .iter()
        .rev()
        .find(|&&s| s <= limit && s > from + RUN_CHARS / 2)
        .copied()
        .unwrap_or(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FindAction, FindBar, Painter, Palette, PaletteAction, PaletteItem, Prompt, PromptInput,
        SUMI, TabEvent, TabInfo, TabStrip,
    };
    use accesskit::{ActionData, ActionRequest};
    use std::time::Instant;

    fn request(action: Action, part: Part, data: Option<ActionData>) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: part.id(),
            data,
        }
    }

    fn click(part: Part) -> ActionRequest {
        request(Action::Click, part, None)
    }

    #[test]
    fn widgets_do_what_assistive_technologies_ask() {
        let mut p = Painter::new(&SUMI);
        p.begin(1.0);
        let now = Instant::now();
        let window = Rect::new(0, 0, 800, 600);

        let mut find = FindBar::new();
        find.open(true, None, now);
        find.draw(&mut p, Rect::new(0, 500, 800, 80), now);
        let mut tree = AccessTree::new();
        find.access(&mut p, &mut tree);
        let case = Part::FindButton(1);
        assert_eq!(tree.get(case).unwrap().label(), Some("Match case"));
        assert_eq!(
            find.access_action(case, &click(case), now),
            Some(FindAction::QueryChanged)
        );
        assert!(find.case_sensitive);
        let field = Part::Field(Field::Find);
        let set = request(
            Action::SetValue,
            field,
            Some(ActionData::Value("abc".into())),
        );
        assert_eq!(
            find.access_action(field, &set, now),
            Some(FindAction::QueryChanged)
        );
        assert_eq!(find.find.text(), "abc");

        let buttons = ["Save", "Don't Save", "Cancel"];
        let mut prompt = Prompt::new("Save changes?", "To Untitled?", &buttons, 0, 2);
        prompt.draw(&mut p, window);
        let mut tree = AccessTree::new();
        prompt.access(&mut tree);
        assert_eq!(
            tree.get(Part::PromptButton(0)).unwrap().role(),
            Role::DefaultButton
        );
        let cancel = Part::PromptButton(2);
        assert_eq!(
            prompt.access_action(cancel, &request(Action::Focus, cancel, None)),
            PromptInput::Handled
        );
        assert_eq!(prompt.access_focus(), cancel);
        let dont = Part::PromptButton(1);
        assert_eq!(
            prompt.access_action(dont, &click(dont)),
            PromptInput::Chose(1)
        );

        let mut palette = Palette::new();
        let item = |title: &str| PaletteItem {
            title: title.to_owned(),
            detail: String::new(),
        };
        palette.open(vec![item("Alpha"), item("Beta")], "", now);
        palette.draw(&mut p, window, 0, now);
        let second = Part::PaletteItem(1);
        assert_eq!(
            palette.access_action(second, &click(second), now),
            Some(PaletteAction::Run(1))
        );

        let mut tabs = TabStrip::new();
        let infos = [
            TabInfo {
                title: "a.txt",
                modified: false,
            },
            TabInfo {
                title: "b.txt",
                modified: true,
            },
        ];
        tabs.draw(&mut p, Rect::new(0, 0, 800, 36), &infos, 0);
        let close = Part::TabClose(1);
        assert_eq!(
            tabs.access_action(close, &click(close)),
            Some(TabEvent::Close(1))
        );
        assert_eq!(
            tabs.access_action(Part::Tab(1), &click(Part::Tab(1))),
            Some(TabEvent::Activate(1))
        );
    }

    #[test]
    fn a_tree_that_isnt_one_becomes_the_bare_window() {
        let check = |nodes: Vec<(Part, Vec<Part>)>, focus: Part| {
            let mut tree = AccessTree::new();
            for (part, children) in nodes {
                let mut node = Node::new(Role::GenericContainer);
                node.set_children(children.iter().map(|c| c.id()).collect::<Vec<_>>());
                tree.push(part, node);
            }
            tree.check(focus.id())
        };
        let good = vec![
            (Part::Window, vec![Part::Editor, Part::Tabs]),
            (Part::Editor, vec![]),
            (Part::Tabs, vec![]),
        ];
        assert_eq!(check(good.clone(), Part::Editor), Ok(()));
        assert!(check(good.clone(), Part::Prompt).is_err(), "focus missing");
        let mut orphan = good.clone();
        orphan.push((Part::Menu, vec![]));
        assert!(check(orphan, Part::Editor).is_err());
        let mut twice = good.clone();
        twice[2].1.push(Part::Editor);
        assert!(check(twice, Part::Editor).is_err());
        let mut missing = good;
        missing[0].1.push(Part::StatusBar);
        assert!(check(missing, Part::Editor).is_err());
    }

    #[test]
    fn ids_round_trip() {
        let parts = [
            Part::Window,
            Part::MenuTitle(3),
            Part::MenuItem(17),
            Part::Tab(0),
            Part::TabClose(9),
            Part::Editor,
            Part::Field(Field::Replace),
            Part::Run(Text::Editor, 40),
            Part::Run(Text::Field(Field::Palette), 2),
            Part::PaletteItem(100),
            Part::PromptButton(1),
        ];
        for part in parts {
            assert_eq!(Part::of(part.id()), Some(part));
        }
        assert_eq!(Part::of(NodeId(0)), None);
    }

    fn row<'a>(bytes: &'a [u8], line_break: &'static [u8]) -> RowText<'a> {
        RowText {
            bytes,
            start: 100,
            line_break,
            before: None,
            y: 10,
            h: 20,
        }
    }

    /// Ten pixels per byte from x = 50.
    fn x_of(at: usize) -> f32 {
        50.0 + (at - 100) as f32 * 10.0
    }

    #[test]
    fn a_row_is_a_run_of_graphemes_with_its_line_break() {
        let mut tree = AccessTree::new();
        let mut runs = TextRuns::default();
        let text = "e\u{301}a b\t\u{0}";
        let row = row(text.as_bytes(), b"\r\n");
        let ids = runs.push_row(&mut tree, Text::Editor, &row, x_of);
        assert_eq!(ids, [Part::Run(Text::Editor, 0).id()]);
        let node = tree.get(Part::Run(Text::Editor, 0)).unwrap();
        assert_eq!(node.value(), Some("e\u{301}a b\t␀\r\n"));
        assert_eq!(node.character_lengths(), &[3, 1, 1, 1, 1, 3, 2]);
        // The control character is one byte in the document but three in the run.
        assert_eq!(runs.runs[0], [100, 103, 104, 105, 106, 107, 108, 110]);
        assert_eq!(
            node.character_positions().unwrap(),
            &[0.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]
        );
        assert_eq!(node.character_widths().unwrap()[6], 0.0);
        // "é", "a ", "b\t" and the control character.
        assert_eq!(node.word_starts(), &[0, 3, 5]);
        let b = node.bounds().unwrap();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (50.0, 10.0, 130.0, 30.0));

        // The caret at the end of the line is on the line break.
        let at_end = runs.position(Text::Editor, 108).unwrap();
        assert_eq!(at_end.character_index, 6);
        assert_eq!(runs.offset(Text::Editor, &at_end), Some(108));
        let after = runs.position(Text::Editor, 110).unwrap();
        assert_eq!(after.character_index, 7);
        assert_eq!(runs.position(Text::Editor, 0).unwrap().character_index, 0);
        assert_eq!(runs.offset(Text::Field(Field::Find), &at_end), None);
    }

    #[test]
    fn long_rows_split_into_linked_runs_at_words() {
        let mut tree = AccessTree::new();
        let mut runs = TextRuns::default();
        let text = "word ".repeat(120);
        let ids = runs.push_row(&mut tree, Text::Editor, &row(text.as_bytes(), b""), x_of);
        assert_eq!(ids.len(), 3);
        let lens: Vec<usize> = (0..3)
            .map(|k| {
                let node = tree.get(Part::Run(Text::Editor, k)).unwrap();
                assert!(node.character_lengths().len() <= RUN_CHARS);
                assert_eq!(node.word_starts().first(), Some(&0));
                node.character_lengths().len()
            })
            .collect();
        assert_eq!(lens.iter().sum::<usize>(), 600);
        assert_eq!(lens[0] % 5, 0, "split at a word");
        let middle = tree.get(Part::Run(Text::Editor, 1)).unwrap();
        assert_eq!(
            middle.previous_on_line(),
            Some(Part::Run(Text::Editor, 0).id())
        );
        assert_eq!(middle.next_on_line(), Some(Part::Run(Text::Editor, 2).id()));
        // The start of the second run is the end of the first.
        let at = 100 + lens[0];
        assert_eq!(
            runs.position(Text::Editor, at).unwrap().node,
            Part::Run(Text::Editor, 1).id()
        );
        // The end of the text is the end of the last run.
        let end = runs.position(Text::Editor, 700).unwrap();
        assert_eq!(end.character_index, lens[2]);
    }

    #[test]
    fn invalid_bytes_and_empty_rows() {
        let mut tree = AccessTree::new();
        let mut runs = TextRuns::default();
        runs.push_row(&mut tree, Text::Editor, &row(b"a\xFFb", b"\n"), x_of);
        let node = tree.get(Part::Run(Text::Editor, 0)).unwrap();
        assert_eq!(node.value(), Some("a\u{FFFD}b\n"));
        assert_eq!(node.character_lengths(), &[1, 3, 1, 1]);
        let empty = RowText {
            start: 104,
            ..row(b"", b"")
        };
        runs.push_row(&mut tree, Text::Editor, &empty, x_of);
        let node = tree.get(Part::Run(Text::Editor, 1)).unwrap();
        assert_eq!(node.value(), Some(""));
        let end = runs.position(Text::Editor, 104).unwrap();
        assert_eq!(end.node, Part::Run(Text::Editor, 1).id());
        assert_eq!(end.character_index, 0);
    }
}
