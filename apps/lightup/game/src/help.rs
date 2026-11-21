//! How to play, in a few pages: each a small grid that shows a rule, drawn
//! as the board draws it, and a sentence or two.  The words are this
//! project's own.

use crate::Grid;

pub struct Page {
    /// The grid of the picture, as [`Grid::parse`] reads it.
    pub picture: &'static str,
    pub text: &'static str,
    /// The text on a phone without keys, where it is another.
    pub touch_text: Option<&'static str>,
}

impl Page {
    pub fn grid(&self) -> Grid {
        Grid::parse(self.picture).expect("a grid")
    }

    /// The page's sentences on a phone with keys (`true`), or on one that
    /// is used by touch alone, which names no key.
    pub fn words(&self, keys: bool) -> &'static str {
        match self.touch_text {
            Some(text) if !keys => text,
            _ => self.text,
        }
    }
}

/// The pictures are five cells by three, so that a page fits the smallest
/// display beside its text.
pub const PAGES: [Page; 5] = [
    Page {
        picture: ".....\n.L.#.\n.....",
        text: "Light every white cell.  A light shines along its row and column until a black cell.",
        touch_text: Some(
            "Light every white cell: touch a cell to put a light on it.  A light shines along its row and column until a black cell.",
        ),
    },
    Page {
        picture: ".....\nL..L.\n.....",
        text: "No light may shine on another light.",
        touch_text: None,
    },
    Page {
        picture: ".L#L.\n.1.1L\n.....",
        text: "A numbered cell needs exactly that many lights next to it.",
        touch_text: None,
    },
    Page {
        picture: ".x0x.\n..x..\n.....",
        text: "A square is your note that a cell gets no light, as next to a 0.",
        touch_text: Some(
            "A square is your note that a cell gets no light, as next to a 0.  Touch a light to make it a square, and a square to clear the cell.",
        ),
    },
    Page {
        picture: ".L...\nx2L..\n.x...",
        text: "8 puts squares around each number that has its lights, and lights where a number has just enough room.",
        touch_text: Some(
            "\"Auto\" puts squares around each number that has its lights, and lights where a number has just enough room.",
        ),
    },
];
