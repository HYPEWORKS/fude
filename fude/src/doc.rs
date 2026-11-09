//! An open document: its editor view and where it lives on disk.

use std::path::PathBuf;
use std::sync::mpsc;

use fude_buffer::{FileStamp, Format, LineEnding, Rope, SaveError, Version};
use fude_editor::Editor;
use fude_ui::EditorView;

/// Identifies a document across tab moves and closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

/// What to do after a save finishes or unsaved changes are dealt with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    Close(DocId),
    Quit,
}

/// A save running on a worker thread.
pub struct SaveJob {
    pub version: Version,
    pub path: PathBuf,
    pub then: Option<Then>,
    pub result: mpsc::Receiver<Result<FileStamp, SaveError>>,
}

pub struct Doc {
    pub id: DocId,
    pub view: EditorView,
    pub path: Option<PathBuf>,
    /// The file as last read or written, to notice changes made by other programs.
    pub stamp: Option<FileStamp>,
    /// Saving would also change bytes the user didn't edit (the encoding can't represent
    /// them exactly); asked about once before the first save.
    pub inexact: bool,
    pub saving: Option<SaveJob>,
    /// The grammar highlighting uses, once the highlighter has picked one.
    pub language: String,
    /// The last highlighting request (version, lines), so it isn't repeated.
    pub requested: Option<(u64, std::ops::Range<usize>)>,
    /// The session backup holding this document's unsaved text, once it has one.
    pub backup: Option<String>,
    /// The file is still being read (the text is a read-only preview of its start).
    pub loading: Option<crate::loading::Loading>,
}

impl Doc {
    pub fn new(id: DocId, text: Rope, format: Format, path: Option<PathBuf>) -> Doc {
        Doc {
            id,
            view: EditorView::new(Editor::new(text, format)),
            path,
            stamp: None,
            inexact: false,
            saving: None,
            language: "Plain Text".to_owned(),
            requested: None,
            backup: None,
            loading: None,
        }
    }

    pub fn untitled(id: DocId) -> Doc {
        let format = Format {
            line_ending: LineEnding::PLATFORM,
            ..Format::default()
        };
        Doc::new(id, Rope::new(), format, None)
    }

    pub fn name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => "Untitled".to_owned(),
        }
    }

    pub fn modified(&self) -> bool {
        self.view.editor.buffer().is_modified()
    }

    /// An untouched new document, which opening a file may replace.
    pub fn is_blank(&self) -> bool {
        self.path.is_none() && !self.modified() && self.view.editor.text().is_empty()
    }
}
