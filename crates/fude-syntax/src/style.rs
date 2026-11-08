//! From grammar scopes ("keyword.control.rust") to a small set of styles the theme colors.

use std::collections::HashMap;

use syntax_scope::{Scope, ScopeStack};

use syntect::parsing as syntax_scope;

/// What a piece of code is, as far as color goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Plain,
    Comment,
    String,
    Escape,
    Number,
    Constant,
    Keyword,
    /// Declaration keywords and modifiers: `fn`, `let`, `struct`, `pub`.
    Storage,
    Operator,
    Function,
    Type,
    Tag,
    Attribute,
    Punctuation,
    Heading,
    Emphasis,
    Link,
    Invalid,
}

/// Scope prefixes in priority order; the first that matches a scope decides its style.
const RULES: &[(&str, Style)] = &[
    ("invalid", Style::Invalid),
    ("comment", Style::Comment),
    ("punctuation.definition.comment", Style::Comment),
    ("constant.character.escape", Style::Escape),
    ("string", Style::String),
    ("markup.raw", Style::String),
    ("constant.numeric", Style::Number),
    ("constant", Style::Constant),
    ("variable.language", Style::Constant),
    ("keyword.operator", Style::Operator),
    ("keyword", Style::Keyword),
    ("storage", Style::Storage),
    ("entity.name.function", Style::Function),
    ("support.function", Style::Function),
    ("variable.function", Style::Function),
    ("entity.name.tag", Style::Tag),
    ("entity.other.attribute-name", Style::Attribute),
    ("entity.name.section", Style::Heading),
    ("markup.heading", Style::Heading),
    ("entity.name", Style::Type),
    ("entity.other.inherited-class", Style::Type),
    ("support.type", Style::Type),
    ("support.class", Style::Type),
    ("markup.bold", Style::Emphasis),
    ("markup.italic", Style::Emphasis),
    ("markup.underline.link", Style::Link),
    ("markup.link", Style::Link),
    ("punctuation", Style::Punctuation),
];

/// Remembers each scope's style; there are only so many scopes.
#[derive(Default)]
pub(crate) struct StyleMap {
    cache: HashMap<Scope, Option<Style>>,
}

impl StyleMap {
    /// The style of the innermost scope that has one.
    pub fn style(&mut self, stack: &ScopeStack) -> Style {
        for scope in stack.as_slice().iter().rev() {
            let style = *self.cache.entry(*scope).or_insert_with(|| {
                let name = scope.build_string();
                RULES
                    .iter()
                    .find(|(prefix, _)| {
                        name == *prefix
                            || name.starts_with(prefix)
                                && name.as_bytes().get(prefix.len()) == Some(&b'.')
                    })
                    .map(|&(_, s)| s)
            });
            if let Some(style) = style {
                return style;
            }
        }
        Style::Plain
    }
}
