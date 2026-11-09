//! How each language writes comments, for toggling them.

/// A language's comment markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comments {
    /// Starts a comment that runs to the end of the line: `//`, `#`.
    pub line: Option<&'static str>,
    /// Opens and closes a comment: `/*` … `*/`.
    pub block: Option<(&'static str, &'static str)>,
}

const C: Comments = Comments {
    line: Some("//"),
    block: Some(("/*", "*/")),
};
const HASH: Comments = Comments {
    line: Some("#"),
    block: None,
};
const MARKUP: Comments = Comments {
    line: None,
    block: Some(("<!--", "-->")),
};

/// The comment markers of a language, by the grammar names the highlighter reports.
/// Plain text (and anything unknown) gets `#`, which most configuration formats without
/// a grammar here use (TOML, INI-style files, shell profiles, `.gitignore`).
pub fn comments(language: &str) -> Comments {
    match language {
        "C" | "C++" | "C#" | "D" | "Go" | "Groovy" | "Java" | "JavaScript"
        | "JavaScript (Rails)" | "Objective-C" | "Objective-C++" | "PHP" | "Rust" | "Scala"
        | "ActionScript" | "Graphviz (DOT)" | "JavaDoc" => C,
        "Python"
        | "Ruby"
        | "Perl"
        | "Bourne Again Shell (bash)"
        | "Makefile"
        | "YAML"
        | "R"
        | "R Console"
        | "Tcl"
        | "Java Properties"
        | "Plain Text" => HASH,
        "SQL" | "SQL (Rails)" => Comments {
            line: Some("--"),
            block: Some(("/*", "*/")),
        },
        "Lua" => Comments {
            line: Some("--"),
            block: Some(("--[[", "]]")),
        },
        "Haskell" | "Literate Haskell" => Comments {
            line: Some("--"),
            block: Some(("{-", "-}")),
        },
        "AppleScript" => Comments {
            line: Some("--"),
            block: Some(("(*", "*)")),
        },
        "Lisp" | "Clojure" => Comments {
            line: Some(";"),
            block: None,
        },
        "Erlang" | "LaTeX" | "TeX" | "BibTeX" | "MATLAB" => Comments {
            line: Some("%"),
            block: None,
        },
        "Batch File" => Comments {
            line: Some("REM"),
            block: None,
        },
        "Pascal" => Comments {
            line: Some("//"),
            block: Some(("{", "}")),
        },
        "OCaml" | "OCamllex" | "OCamlyacc" | "camlp4" => Comments {
            line: None,
            block: Some(("(*", "*)")),
        },
        "CSS" => Comments {
            line: None,
            block: Some(("/*", "*/")),
        },
        "HTML"
        | "HTML (ASP)"
        | "HTML (Erlang)"
        | "HTML (Rails)"
        | "HTML (Tcl)"
        | "XML"
        | "Markdown"
        | "MultiMarkdown"
        | "Java Server Page (JSP)"
        | "ASP"
        | "NAnt Build File" => MARKUP,
        "JSON"
        | "Diff"
        | "Textile"
        | "Regular Expression"
        | "reStructuredText"
        | "LaTeX Log"
        | "Rd (R Documentation)"
        | "Ruby Haml"
        | "Ruby on Rails" => Comments {
            line: None,
            block: None,
        },
        _ => HASH,
    }
}
